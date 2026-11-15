//! Nothing the server keeps holds a run's data in clear (docs/L4s/at-rest-encryption-L4.md,
//! step 6), against the real server: `holon serve` as it is installed, flows deployed with `holon
//! deploy` that leave a canary in every kind of file a run leaves, then every file under its home
//! and its backup folder read, and every gzip or deflate stream found in one opened. And the
//! server's sources: no writer under the data folder goes around at_rest.rs. A test binary of its
//! own, as owner_only.rs.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

const HOLON: &str = env!("CARGO_BIN_EXE_holon");

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

fn post(port: u16, path: &str, body: &str) -> String {
    raw_http(port, format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes())
}

fn get(port: u16, path: &str) -> String {
    raw_http(port, format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes())
}

/// An answer's status, 0 when there is none.
fn status(answered: &str) -> u16 {
    answered.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Every file under `dir`, folders walked into.
fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Wait up to `secs` for `done`, saying `what` if it never is.
fn until(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let by = Instant::now() + Duration::from_secs(secs);
    while !done() {
        assert!(Instant::now() < by, "{what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn b64(bytes: &[u8], alphabet: &[u8; 64]) -> String {
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..=c.len() {
            out.push(alphabet[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// Each form the canary may take in a file: as it is (and so inside JSON, which escapes nothing
/// of it), in hex either case, and in base64 (standard or URL-safe) at each of the three places
/// it may start in a group of three bytes: the stretch of its encoding no neighbour changes.
fn forms(canary: &str) -> Vec<Vec<u8>> {
    let mut out = vec![canary.as_bytes().to_vec()];
    let hex: String = canary.bytes().map(|b| format!("{b:02x}")).collect();
    out.push(hex.clone().into_bytes());
    out.push(hex.to_uppercase().into_bytes());
    const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    for alphabet in [STANDARD, URL] {
        for lead in 0..3 {
            let mut bytes = vec![b'x'; lead];
            bytes.extend_from_slice(canary.as_bytes());
            let encoded = b64(&bytes, alphabet);
            // The characters that depend only on the canary's bytes: past the lead's group, short
            // of the last (which a byte after it changes).
            let from = if lead == 0 { 0 } else { 4 };
            let to = (bytes.len() / 3) * 4;
            out.push(encoded.as_bytes()[from..to].to_vec());
        }
    }
    out
}

fn holds(bytes: &[u8], forms: &[Vec<u8>]) -> Option<usize> {
    forms.iter().position(|f| bytes.windows(f.len()).any(|w| w == &f[..]))
}

/// What of `bytes` holds the canary, said: the bytes themselves, or a gzip or deflate stream in
/// them, opened (a segment, an archive, a pack's blocks, were one not sealed).
fn found_in(bytes: &[u8], forms: &[Vec<u8>]) -> Option<String> {
    if let Some(i) = holds(bytes, forms) {
        return Some(format!("as it is (form {i})"));
    }
    let mut streams: Vec<(usize, bool)> = bytes.windows(3).enumerate().filter(|(_, w)| *w == [0x1f, 0x8b, 0x08]).map(|(i, _)| (i, true)).collect();
    if bytes.starts_with(b"HOLONPK") {
        streams.push((8, false));
    }
    for (at, gzip) in streams {
        let mut inflated = Vec::new();
        let read = if gzip {
            flate2::read::GzDecoder::new(&bytes[at..]).take(256 << 20).read_to_end(&mut inflated)
        } else {
            flate2::read::DeflateDecoder::new(&bytes[at..]).take(256 << 20).read_to_end(&mut inflated)
        };
        if (read.is_ok() || !inflated.is_empty())
            && let Some(i) = holds(&inflated, forms)
        {
            return Some(format!("in a {} stream at {at} (form {i})", if gzip { "gzip" } else { "deflate" }));
        }
    }
    None
}

/// A mail server that greets, takes a binding's check, and puts off every mail (451): what is
/// mailed stays in the spool, tried again later. Its port.
fn putting_off_smtp() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut out = stream;
                let _ = out.write_all(b"220 stand-in ESMTP\r\n");
                let mut line = String::new();
                while std::io::BufRead::read_line(&mut reader, &mut line).is_ok_and(|n| n > 0) {
                    let verb = line.get(..4).unwrap_or_default().to_ascii_uppercase();
                    let answer: &[u8] = match verb.as_str() {
                        "EHLO" => b"250-stand-in\r\n250 AUTH PLAIN LOGIN\r\n",
                        "AUTH" => b"235 ok\r\n",
                        "MAIL" => b"451 4.3.0 not now\r\n",
                        "QUIT" => b"221 bye\r\n",
                        _ => b"250 ok\r\n",
                    };
                    if out.write_all(answer).is_err() || verb == "QUIT" {
                        break;
                    }
                    line.clear();
                }
            });
        }
    });
    port
}

/// A queue service (SQS's JSON protocol) that answers a binding's check and puts off every
/// message (500): what is sent stays in the spool, tried again later. Its port.
fn putting_off_sqs() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            // The head, then as many bytes of body as it says.
            let head = loop {
                let Ok(n) = stream.read(&mut buf) else { break String::new() };
                if n == 0 {
                    break String::new();
                }
                got.extend_from_slice(&buf[..n]);
                if let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&got[..end]).to_ascii_lowercase();
                    let len: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                    // The body may come after the head, in reads of its own.
                    while got.len() < end + 4 + len {
                        match stream.read(&mut buf) {
                            Ok(n) if n > 0 => got.extend_from_slice(&buf[..n]),
                            _ => break,
                        }
                    }
                    break head;
                }
            };
            let (code, body) = if head.contains("amazonsqs.getqueueattributes") {
                ("200 OK", r#"{"Attributes":{"VisibilityTimeout":"30","MaximumMessageSize":"262144"}}"#)
            } else {
                ("500 Internal Server Error", r#"{"__type":"com.amazonaws.sqs#InternalError","message":"not now"}"#)
            };
            let _ = stream.write_all(format!("HTTP/1.1 {code}\r\nContent-Type: application/x-amz-json-1.0\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            // Closed gently: what the client still sends is read to its end, so the close is no
            // reset that loses the answer.
            let _ = stream.shutdown(std::net::Shutdown::Write);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            while stream.read(&mut buf).is_ok_and(|n| n > 0) {}
        }
    });
    port
}

/// The server, its home and its folders: stopped and taken away when dropped.
struct Server {
    child: Child,
    dirs: Vec<PathBuf>,
    port: u16,
    home: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for d in &self.dirs {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

impl Server {
    /// `holon <args>` as this server's own command line: its home, its port.
    fn holon(&self, args: &[&str]) -> String {
        let at = self.port.to_string();
        let mut all: Vec<&str> = args.to_vec();
        all.extend(["--server", &at]);
        let out = Command::new(HOLON).args(&all).env("HOLON_HOME", &self.home).env_remove("HOLON_DATA").env_remove("HOLON_RUN_DIR").output().expect("holon runs");
        assert!(out.status.success(), "holon {args:?}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Stopped as a person stops it: told to (SIGTERM) where a test can, so every line is on disk
    /// and its clean-stop mark is written; on Windows, killed (a console's ctrl+c can't be sent to
    /// it from here), which leaves what a crash leaves.
    fn stop(&mut self) {
        #[cfg(unix)]
        {
            let _ = Command::new("kill").args(["-TERM", &self.child.id().to_string()]).status();
            let by = Instant::now() + Duration::from_secs(30);
            while self.child.try_wait().unwrap().is_none() && Instant::now() < by {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Guards: no file the server keeps holds what a run was given or the flow's own text, in any
/// form: not a table (the deployments hold the flow's document), a log
/// line or segment, the store's units, values, packs or file texts, a spool, a removed
/// deployment's records, a backup's batch or archive; nor the files kept in clear on purpose
/// (§2.5: the state head, the witness's queue, the policy, the clean-stop mark).
/// Why: each was a writer of its own, sealing with at_rest.rs on its own path; one that stops (a
/// new writer, a change that writes past it) leaves run data in clear for anyone with the folder.
/// If it fails: the file it names, and how the canary is in it, is a writer not sealing; the
/// control at the end failing means the canary never reached the record, and the test proves
/// nothing until it does.
#[test]
fn no_file_the_server_writes_holds_run_data_in_clear() {
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
    let canary = format!("canary-{:016x}", stamp as u64 ^ u64::from(std::process::id()).rotate_left(32));
    let base = std::env::temp_dir().join(format!("holon-at-rest-{}-{stamp}", std::process::id()));
    let (home, backup, outside, folder) = (base.join("home"), base.join("backup"), base.join("texts"), base.join("box"));
    for d in [&home, &backup, &outside, &folder] {
        std::fs::create_dir_all(d).unwrap();
    }
    // The file a run reads from a bound folder: its text is kept for the log, past 4 KiB apart
    // from it (folders.rs LOG_INLINE_BYTES, blobs.rs).
    let text: String = (0..400).map(|i| format!("line {i:04} of a file that says {canary}\n")).collect();
    std::fs::write(folder.join("read.txt"), text).unwrap();
    let port = free_port();
    // Mail and a queue bound to servers that take the binding's check and put off every send:
    // what a run sends waits in its spool, tried again later.
    let (smtp, sqs) = (putting_off_smtp(), putting_off_sqs());
    let child = Command::new(HOLON)
        .args(["serve", "--at", &format!("127.0.0.1:{port}"), "--root-key", &format!("file:{}", base.join("root.key").display())])
        .env("HOLON_HOME", &home)
        .env("HOLON_SECRET_OPS_SMTP", format!("smtp://127.0.0.1:{smtp}"))
        .env("HOLON_SECRET_OUT_URL", format!("http://k:s@127.0.0.1:{sqs}/000000000000/out?region=stand-in"))
        .env_remove("HOLON_DATA")
        .env_remove("HOLON_RUN_DIR")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("holon serve starts");
    let mut server = Server { child, dirs: vec![base.clone()], port, home: home.clone() };
    until("holon serve answers", 60, || TcpStream::connect(("127.0.0.1", port)).is_ok());

    server.holon(&["policy", "--backup", &backup.display().to_string(), "--sweep-every", "1m"]);
    server.holon(&["folder", "add", "box", &folder.display().to_string(), "--read-only"]);
    server.holon(&["mail", "add", "ops", "--smtp", "env:HOLON_SECRET_OPS_SMTP", "--from", "Ops <ops@acme.com>", "--to", "*@acme.com"]);
    server.holon(&["queue", "add", "out", "--url", "env:HOLON_SECRET_OUT_URL"]);

    // One flow through every writer a run reaches: state, a collection, console lines, a run left
    // waiting, a file read, mail and a queue message on their way; runs enough that its log seals
    // its older ones into segments and lets go of the oldest to the backup.
    let (kept_port, gone_port) = (free_port(), free_port());
    let kept = format!(
        r#"import {{ action, event, flow, route }} from "holon"
import state from "state"
import notes from "collection:notes"
import box from "folder:box"
import ops from "mail:ops"
import out from "queue:out"

flow("kept", {{ port: {kept_port}, keep: {{ runs: 200 }} }})

route.post("/note", (req) => {{
  console.log("noted", req.json.text)
  state.last = req.json.text
  notes[req.json.text] = {{ text: req.json.text }}
  const started = waits(req.json)
  return `/done/${{started.waiting.done}}`
}})

route.post("/bump", (req) => {{
  state.n = (state.n ?? 0) + 1
  state.seen = req.json.text
  return state.n
}})

route.post("/read", () => box["read.txt"])

route.post("/send", (req) => {{
  ops.send({{ to: "someone@acme.com", subject: req.json.text, text: req.json.text }})
  out.send({{ data: {{ text: req.json.text }} }})
  return "sent"
}})

export const waits = action(async (note) => {{
  const done = await event("done", {{ expires: "1d" }})
  state.done = {{ text: note.text, how: done.how }}
}})

route.get("/done/:token", () => "done", {{ resumes: "done" }})

// The flow's own text is the owner's too: kept in the deployments table and the store's units.
route.get("/about", () => "{canary}")
"#
    );
    // And one removed: its records kept as an export (deploy.rs keep_records).
    let gone = format!(
        r#"import {{ flow, route }} from "holon"
import state from "state"

flow("gone", {{ port: {gone_port} }})

route.post("/keep", (req) => {{
  state.text = req.json.text
  return "kept"
}})
"#
    );
    // And one keeping few runs, with none left waiting (a waiting run holds its log's runs after
    // it): what its cut-backs let go of goes to the backup, batched, then archived.
    let rolled_port = free_port();
    let rolled = format!(
        r#"import {{ flow, route }} from "holon"
import state from "state"

flow("rolled", {{ port: {rolled_port}, keep: {{ runs: 20 }} }})

route.post("/keep", (req) => {{
  state.text = req.json.text
  return "kept"
}})
"#
    );
    for (name, text) in [("kept", &kept), ("gone", &gone), ("rolled", &rolled)] {
        let file = outside.join(format!("{name}.holon.ts"));
        std::fs::write(&file, text).unwrap();
        server.holon(&["deploy", &file.display().to_string(), "--approve"]);
    }
    let body = format!(r#"{{"text":"{canary}"}}"#);
    until("kept answers", 30, || status(&post(kept_port, "/bump", &body)) == 200);
    until("gone answers", 30, || status(&post(gone_port, "/keep", &body)) == 200);
    until("rolled answers", 30, || status(&post(rolled_port, "/keep", &body)) == 200);
    for _ in 0..100 {
        assert_eq!(status(&post(rolled_port, "/keep", &body)), 200);
    }
    let answered = post(kept_port, "/note", &body);
    assert_eq!(status(&answered), 200, "{answered}");
    let resume = answered.rsplit("\r\n\r\n").next().unwrap().trim_matches('"').to_string();
    assert_eq!(status(&post(kept_port, "/note", &body)), 200, "a run left waiting");
    assert_eq!(status(&get(kept_port, &resume)), 200, "the first run resumed at {resume}");
    let read = post(kept_port, "/read", &body);
    assert!(status(&read) == 200 && read.contains(&canary), "the file read: {read}");
    assert_eq!(status(&post(kept_port, "/send", &body)), 200);
    for _ in 0..300 {
        assert_eq!(status(&post(kept_port, "/bump", &body)), 200);
    }
    server.holon(&["stop", "gone"]);
    server.holon(&["remove", "gone", "--yes"]);

    // The store's values aged past what a sweep spares, so the server's own sweep (each minute)
    // packs the ones still named.
    let values = home.join("data").join("store").join("values");
    let old = SystemTime::now() - Duration::from_secs(3 * 60 * 60);
    for f in std::fs::read_dir(&values).unwrap().flatten().filter(|e| e.path().is_file()) {
        std::fs::File::options().write(true).open(f.path()).and_then(|f| f.set_modified(old)).unwrap();
    }
    let has = |dir: &Path, pred: &dyn Fn(&str) -> bool| {
        let mut all = Vec::new();
        files_under(dir, &mut all);
        all.iter().any(|f| pred(&f.to_string_lossy().replace('\\', "/")))
    };
    until("a pack written", 150, || has(&values, &|f| f.ends_with(".pack")));
    until("a backup archive made", 60, || has(&backup, &|f| f.ends_with(".json.gz")));
    // Console lines are written about once a second, on a thread of their own (logs.rs).
    std::thread::sleep(Duration::from_millis(2500));

    // The control: the canary reached the record, so its absence below means something.
    let export = base.join("export");
    std::fs::create_dir_all(&export).unwrap();
    server.holon(&["export", "kept", "--to", &export.display().to_string()]);
    let mut exported = Vec::new();
    files_under(&export, &mut exported);
    let forms = forms(&canary);
    assert!(exported.iter().any(|f| found_in(&std::fs::read(f).unwrap(), &forms).is_some()), "the export holds the canary: {exported:#?}");
    server.stop();

    let mut files = Vec::new();
    files_under(&home, &mut files);
    files_under(&backup, &mut files);
    let named = |pred: &dyn Fn(&str) -> bool| files.iter().any(|f| pred(&f.to_string_lossy().replace('\\', "/")));
    // The control again: every kind of file this means to look at is there.
    for (what, found) in [
        ("the run log", named(&|f| f.ends_with("deployments.logs/kept.jsonl"))),
        ("a sealed segment", named(&|f| f.contains("deployments.logs/kept.sealed/") && f.ends_with(".jsonl.gz"))),
        ("console lines", named(&|f| f.ends_with("deployments.logs/kept.console.jsonl"))),
        ("events", named(&|f| f.ends_with("deployments.logs/kept.events.jsonl"))),
        ("the deployments", named(&|f| f.ends_with("data/deployments.json"))),
        ("the activity", named(&|f| f.ends_with("data/deployments.activity.jsonl"))),
        ("the state head", named(&|f| f.ends_with("deployments.state/kept"))),
        ("the store's units", named(&|f| f.contains("data/store/") && !f.contains("/values/") && !f.contains("/blobs/"))),
        ("the store's values", named(&|f| f.contains("data/store/values/") && !f.ends_with(".pack"))),
        ("a pack", named(&|f| f.contains("data/store/values/packs/") && f.ends_with(".pack"))),
        ("a file's text kept", named(&|f| f.contains("data/store/blobs/chunks/"))),
        ("mail on its way", named(&|f| f.contains("/mail-spool/"))),
        ("a queue message on its way", named(&|f| f.contains("/queue-spool/"))),
        ("a removed deployment's records", named(&|f| f.contains("deployments.logs/removed/gone-runs-"))),
        ("a backup's archive", named(&|f| f.contains("/backup/") && f.contains("/holon-archive-") && f.ends_with(".json.gz"))),
        ("the policy", named(&|f| f.ends_with("holon.toml"))),
    ] {
        assert!(found, "{what} is among the files looked at: {files:#?}");
    }
    let clear: Vec<String> = files.iter().filter_map(|f| found_in(&std::fs::read(f).unwrap(), &forms).map(|how| format!("{}: {how}", f.display()))).collect();
    assert!(clear.is_empty(), "the canary {canary} in clear, of {} files: {clear:#?}", files.len());
}

/// `holon serve` on `home`, its root where `root` says (`file:…`), at `port`.
fn serve(home: &Path, root: &str, port: u16, stderr: Stdio) -> Child {
    Command::new(HOLON)
        .args(["serve", "--at", &format!("127.0.0.1:{port}"), "--root-key", root])
        .env("HOLON_HOME", home)
        .env_remove("HOLON_DATA")
        .env_remove("HOLON_RUN_DIR")
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .expect("holon serve starts")
}

/// `holon <args>` with no server named (the offline commands): whether it went, and what it said.
fn offline(home: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(HOLON).args(args).env("HOLON_HOME", home).env_remove("HOLON_DATA").env_remove("HOLON_RUN_DIR").output().expect("holon runs");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// Guards, against the real server: a deployment's data key rotated, its chain still checks
/// across the two versions; its keys forgotten with `holon remove --forget`, its kept records say
/// so, and the old root (rotated in place) opens nothing of the folder; the root rotated with
/// the server stopped, and not while it runs, and in place however `--to` spells the file; the
/// root exported from where the running server reads it; a second server on one folder refuses
/// to start.
/// Why: rotation and forgetting are what an operator does when a key may have leaked or a
/// customer leaves (docs/L4s/at-rest-encryption-L4.md §2.4); each is a few writes to keys.json
/// and the root's place, which the unit tests check one by one, and this one through the commands
/// people run.
/// If it fails: the step it names; a hang at "a second server" means the folder lock is gone.
#[test]
fn keys_are_rotated_and_forgotten_on_the_real_server() {
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
    let base = std::env::temp_dir().join(format!("holon-keys-{}-{stamp}", std::process::id()));
    let (home, texts, export) = (base.join("home"), base.join("texts"), base.join("export"));
    for d in [&home, &texts, &export] {
        std::fs::create_dir_all(d).unwrap();
    }
    let root_file = base.join("root.key");
    let root = format!("file:{}", root_file.display());
    let port = free_port();
    let mut server = Server { child: serve(&home, &root, port, Stdio::null()), dirs: vec![base.clone()], port, home: home.clone() };
    until("holon serve answers", 60, || TcpStream::connect(("127.0.0.1", port)).is_ok());
    let shop_port = free_port();
    let file = texts.join("shop.holon.ts");
    std::fs::write(
        &file,
        format!(
            r#"import {{ flow, route }} from "holon"
import state from "state"

flow("shop", {{ port: {shop_port} }})

route.post("/bump", () => {{
  state.n = (state.n ?? 0) + 1
  return state.n
}})
"#
        ),
    )
    .unwrap();
    server.holon(&["deploy", &file.display().to_string(), "--approve"]);
    until("shop answers", 30, || status(&post(shop_port, "/bump", "{}")) == 200);
    for _ in 0..3 {
        assert_eq!(status(&post(shop_port, "/bump", "{}")), 200);
    }

    // A new data key for the deployment: what it writes from now on is sealed under version 2,
    // and its chain checks across both.
    let rotated = server.holon(&["key", "rotate", "d:shop"]);
    assert!(rotated.contains("now version 2"), "{rotated}");
    for _ in 0..3 {
        assert_eq!(status(&post(shop_port, "/bump", "{}")), 200);
    }
    let shown = server.holon(&["key", "status"]);
    assert!(shown.contains("d:shop: version 2") && shown.contains("names: version 1, never rotated") && shown.contains("root.key"), "{shown}");
    // The root exported from where the running server reads it: --server and no --root-key.
    // Looking in the default place instead found nothing for a server given --root-key file:…
    let copy = base.join("root.copy");
    let said = server.holon(&["key", "export", "--to", &format!("file:{}", copy.display())]);
    assert!(said.contains("is copied to"), "{said}");
    assert!(std::fs::read(&copy).unwrap() == std::fs::read(&root_file).unwrap(), "the copy isn't the root the server reads");
    server.holon(&["export", "shop", "--to", &export.display().to_string()]);
    let mut exported = Vec::new();
    files_under(&export, &mut exported);
    let json = exported.iter().find(|f| f.extension().is_some_and(|e| e == "json")).unwrap_or_else(|| panic!("an export: {exported:#?}"));
    let (checks, said) = offline(&home, &["verify", &json.display().to_string()]);
    assert!(checks, "the chain across the rotation checks: {said}");

    // A second server on the same folder refuses to start, rather than write keys.json over this
    // one's; and the root isn't rotated under a running server.
    let mut second = serve(&home, &root, free_port(), Stdio::piped());
    let by = Instant::now() + Duration::from_secs(30);
    while second.try_wait().unwrap().is_none() {
        if Instant::now() > by {
            let _ = second.kill();
            panic!("a second server on the folder started");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut refused = String::new();
    second.stderr.take().unwrap().read_to_string(&mut refused).unwrap();
    assert!(!second.wait().unwrap().success() && refused.contains("a server is running"), "{refused}");
    let new_root = format!("file:{}", base.join("new.key").display());
    let (went, said) = offline(&home, &["key", "rotate", "--root", "--to", &new_root, "--root-key", &root]);
    assert!(!went && said.contains("a server is running"), "{said}");

    // Removed, its keys forgotten, the root rotated where it is kept.
    let old_root = std::fs::read(&root_file).unwrap();
    server.holon(&["stop", "shop"]);
    let forgot = server.holon(&["remove", "shop", "--yes", "--forget"]);
    assert!(forgot.contains("its keys are forgotten") && forgot.contains("the root key is rotated"), "{forgot}");
    assert_ne!(std::fs::read(&root_file).unwrap(), old_root, "the root rotated in place");
    let shown = server.holon(&["key", "status"]);
    assert!(shown.contains("d:shop: forgotten") && !shown.contains("d:shop: version"), "{shown}");
    server.stop();

    // Its records, kept when it was removed, say they were forgotten.
    let mut files = Vec::new();
    files_under(&home, &mut files);
    let records = files.iter().find(|f| f.to_string_lossy().replace('\\', "/").contains("removed/shop-runs-")).unwrap_or_else(|| panic!("its records: {files:#?}"));
    let (opened, said) = offline(&home, &["verify", &records.display().to_string(), "--root-key", &root]);
    assert!(!opened && said.contains("forgotten"), "{said}");
    // The old root, as a copy of it kept elsewhere would be, opens nothing of the folder now.
    let old_file = base.join("old.key");
    std::fs::write(&old_file, &old_root).unwrap();
    let (went, said) = offline(&home, &["key", "rotate", "--root", "--to", &new_root, "--root-key", &format!("file:{}", old_file.display())]);
    assert!(!went && said.contains("different root key"), "{said}");

    // The root rotated with the server stopped, to a new place, the old one forgotten there: the
    // server starts again under the new root, and keeps what it knew.
    let (went, said) = offline(&home, &["key", "rotate", "--root", "--to", &new_root, "--forget-old", "--root-key", &root]);
    assert!(went && said.contains("the old root key is gone"), "{said}");
    assert!(!root_file.exists(), "the old root removed from its file");
    let port = free_port();
    let mut again = Server { child: serve(&home, &new_root, port, Stdio::null()), dirs: Vec::new(), port, home: home.clone() };
    until("holon serve answers under the new root", 60, || TcpStream::connect(("127.0.0.1", port)).is_ok());
    let shown = again.holon(&["key", "status"]);
    assert!(shown.contains("new.key") && shown.contains("d:shop: forgotten"), "{shown}");
    again.stop();

    // Rotated in place, --to the key file in use spelled another way (and in another case where
    // the disk ignores it): the new key goes in the file as it is spelled where it is kept, and
    // that is what the server is told to start with.
    let other = texts.join("..").join(if cfg!(windows) { "NEW.KEY" } else { "new.key" });
    let (went, said) = offline(&home, &["key", "rotate", "--root", "--to", &format!("file:{}", other.display()), "--root-key", &new_root]);
    assert!(went && said.contains(&format!("start the server with --root-key {new_root}\n")), "{said}");
    let names: Vec<String> = std::fs::read_dir(&base).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(names.iter().any(|n| n == "new.key") && !names.iter().any(|n| n == "NEW.KEY"), "{names:?}");
}

/// The low-level writers (owner-only, atomic, appending) and the standard library's, outside
/// test code: each file's count of them, and why it may write past at_rest.rs.
const WRITERS: &[(&str, usize, &str)] = &[
    ("at_rest.rs", 8, "where sealing is: it writes what it sealed, keys.lock, empty, and keys.json's copy in the backup folder, by the archiver and as keys.json is written: data keys wrapped under the root"),
    ("bindings.rs", 1, "access.versions.jsonl: lines sealed with seal_line just before"),
    ("blobs.rs", 2, "the file texts' chunks and chunk lists, sealed with seal just before"),
    ("claims.rs", 1, "the state head: a hash, in clear on purpose (§2.5)"),
    ("events.rs", 1, "the events file opened to append lines sealed by its LineSeal"),
    ("flowcmd.rs", 1, "the command line writing a flow's text where a person asked"),
    ("flowscmd.rs", 1, "the command line writing a saved flow's text where a person asked"),
    ("folders.rs", 2, "a flow's writes into a bound folder: the person's own folder, not the server's"),
    ("heads.rs", 3, "the witness's queue and receipts: heads and batch ids, in clear on purpose (§2.5)"),
    ("local.rs", 2, "the command line's own files: where a server is, its sign-in"),
    ("locks.rs", 1, "a lock file: empty"),
    ("logs.rs", 1, "console lines, sealed by its LineSeal before they are appended"),
    ("mcphosts.rs", 1, "an MCP client's config file, written where a person asked"),
    ("migrate.rs", 1, "holon migrate: key files an older release kept in the config folder, moved as they are"),
    ("recorder.rs", 7, "the run log: its lines and segments sealed (LineSeal) before any of these writes them"),
    ("sealing.rs", 5, "the writers themselves, the root key's own file, and the keys files listed beside it: paths, kept with the root, never in the data folder"),
    ("selfmanage.rs", 1, "holon's own install, edited on update"),
    ("stopping.rs", 1, "the clean-stop mark: a sentence, in clear on purpose (§2.5)"),
    ("storage.rs", 2, "the policy (holon.toml): hand-edited, in clear on purpose (§2.5); and a removed deployment's records moved to the backup, sealed already, byte for byte (R41)"),
    ("verify.rs", 2, "holon export: the plain export a person asked for, where they asked (§6)"),
    ("workbench/app.rs", 1, "the terminal app writing what a person saved"),
    ("workbench/flow.rs", 1, "the terminal app writing a deployment's export where a person asked"),
];

/// Guards: nothing under the data folder is written past at_rest.rs but the writers named, each
/// with its reason.
/// Why: the class test above sees the files one server run leaves; a writer added later, on a
/// path that test doesn't take, would leave run data in clear unseen. A new call to a low-level
/// writer anywhere must say why it may.
/// If it fails: seal what it writes through at_rest.rs (`write_table`, `write_sealed`,
/// `LineSeal`), or, if it writes nothing a run gave (a person's own file, a hash), add it to
/// WRITERS with why.
#[test]
fn no_writer_under_the_data_folder_bypasses_at_rest() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    files_under(&src, &mut files);
    let mut found = std::collections::BTreeMap::<String, usize>::new();
    for f in files.iter().filter(|f| f.extension().is_some_and(|e| e == "rs")) {
        let name = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        if name.ends_with("tests.rs") || name.contains("/tests/") || name.starts_with("tests/") {
            continue;
        }
        let text = std::fs::read_to_string(f).unwrap();
        // Test code (a `#[cfg(test)]` module, to the end of the file, as they are here) aside.
        let code = ["\n#[cfg(test)]\nmod ", "\n#[cfg(test)]\npub(crate) mod "].iter().filter_map(|m| text.find(m)).min().map_or(&text[..], |at| &text[..at]);
        let mut n = 0;
        for line in code.lines().map(str::trim).filter(|l| !l.starts_with("//")) {
            for writer in ["write_private(", "write_aside(", "append_private(", "write_atomically("] {
                let mut rest = line;
                while let Some(at) = rest.find(writer) {
                    let before = &rest[..at];
                    // A definition, and a call to at_rest's own sealing writer, aren't one of these.
                    if !before.ends_with("fn ") && !before.ends_with("at_rest::") && !before.ends_with("_") {
                        n += 1;
                    }
                    rest = &rest[at + writer.len()..];
                }
            }
            n += line.matches("OpenOptions::new(").count() + line.matches("fs::write(").count();
        }
        if n > 0 {
            found.insert(name, n);
        }
    }
    let allowed: std::collections::BTreeMap<String, usize> = WRITERS.iter().map(|(f, n, _)| (f.to_string(), *n)).collect();
    assert_eq!(found, allowed, "a low-level writer added or gone: seal through at_rest.rs, or say in WRITERS why it may write past it");
}
