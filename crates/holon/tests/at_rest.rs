//! Nothing the server keeps holds a run's data in clear (docs/L4s/at-rest-encryption-L4.md,
//! step 6), against the real server: `holon serve` as it is installed, flows deployed with `holon
//! deploy` that leave a canary in every kind of file a run leaves, then every file under its home
//! and its backup folder read, and every gzip or deflate stream found in one opened. And the
//! server's sources: no writer under the data folder goes around at_rest.rs. A test binary of its
//! own, as owner_only.rs.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

mod common;
use common::{answering, free_port, raw_http, status, EVENTUALLY};
/// Which lines of a source file are a test module's (R78): the server's tests ask it too.
#[path = "../src/tests/source.rs"]
mod source;

/// A kind of file the test means to look at: what it is, and how its path (with `/`) is known.
type Kind = (&'static str, fn(&str) -> bool);

const HOLON: &str = env!("CARGO_BIN_EXE_holon");

fn post(port: u16, path: &str, body: &str) -> String {
    raw_http(port, format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes())
}

fn get(port: u16, path: &str) -> String {
    raw_http(port, format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes())
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

/// Wait up to `secs` for `done` ([`EVENTUALLY`] at least), saying `what` if it never is.
fn until(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let by = Instant::now() + Duration::from_secs(secs).max(EVENTUALLY);
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
/// mailed stays in the spool, tried again later; but one to a recipient whose name starts with
/// `given-up-` is refused for good (550), and given up. Its port.
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
                        "RCPT" if line.contains("given-up-") => b"550 5.1.1 no such user\r\n",
                        "RCPT" => b"451 4.3.0 not now\r\n",
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
            // not a bound: past it the stand-in stops reading what the client still sends
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
            // not a bound: past it the server is killed, which leaves what a crash leaves, as this stop may
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
    let said = base.join("console.err");
    let child = Command::new(HOLON)
        .args(["serve", "--at", &format!("127.0.0.1:{port}"), "--root-key", &format!("file:{}", base.join("root.key").display())])
        .env("HOLON_HOME", &home)
        .env("HOLON_SECRET_OPS_SMTP", format!("smtp://127.0.0.1:{smtp}"))
        .env("HOLON_SECRET_OUT_URL", format!("http://k:s@127.0.0.1:{sqs}/000000000000/out?region=stand-in"))
        .env_remove("HOLON_DATA")
        .env_remove("HOLON_RUN_DIR")
        // What it says on its console too, which a collector (journald, docker logs) keeps in
        // clear: nothing a run was given belongs there.
        .stdout(std::fs::File::create(base.join("console.out")).unwrap())
        .stderr(std::fs::File::create(&said).unwrap())
        .spawn()
        .expect("holon serve starts");
    let mut server = Server { child, dirs: vec![base.clone()], port, home: home.clone() };
    let console = || ["console.out", "console.err"].map(|f| std::fs::read(base.join(f)).unwrap_or_default()).concat();
    answering(&mut server.child, port, &said);

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
  ops.send({{ to: `given-up-${{req.json.text}}@acme.com`, subject: req.json.text, text: req.json.text }})
  ops.send({{ to: `${{req.json.text}}@elsewhere.example`, subject: req.json.text, text: req.json.text }})
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
    until("kept answers", 60, || status(&post(kept_port, "/bump", &body)) == 200);
    until("gone answers", 60, || status(&post(gone_port, "/keep", &body)) == 200);
    until("rolled answers", 60, || status(&post(rolled_port, "/keep", &body)) == 200);
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
    // One mail refused for good by the mail server, one to a recipient the binding doesn't allow:
    // each given up, and said on the console.
    let given_up = || String::from_utf8_lossy(&console()).matches("gave up a mail").count();
    until("two mails given up, said", 60, || given_up() == 2);
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
    // The control: every kind of file this means to look at, each looked for until it is there,
    // not slept for, and again once the server has stopped. Some are written on their own time:
    // console lines about once a second on a thread of their own (logs.rs), a sealed segment once
    // a cut seals the older runs (recorder.rs).
    let kinds: [Kind; 16] = [
        ("the run log", |f| f.ends_with("deployments.logs/kept.jsonl")),
        ("a sealed segment", |f| f.contains("deployments.logs/kept.sealed/") && f.ends_with(".jsonl.gz")),
        ("console lines", |f| f.ends_with("deployments.logs/kept.console.jsonl")),
        ("events", |f| f.ends_with("deployments.logs/kept.events.jsonl")),
        ("the deployments", |f| f.ends_with("data/deployments.json")),
        ("the activity", |f| f.ends_with("data/deployments.activity.jsonl")),
        ("the state head", |f| f.ends_with("deployments.state/kept")),
        ("the store's units", |f| f.contains("data/store/") && !f.contains("/values/") && !f.contains("/blobs/")),
        ("the store's values", |f| f.contains("data/store/values/") && !f.ends_with(".pack")),
        ("a pack", |f| f.contains("data/store/values/packs/") && f.ends_with(".pack")),
        ("a file's text kept", |f| f.contains("data/store/blobs/chunks/")),
        ("mail on its way", |f| f.contains("/mail-spool/")),
        ("a queue message on its way", |f| f.contains("/queue-spool/")),
        ("a removed deployment's records", |f| f.contains("deployments.logs/removed/gone-runs-")),
        ("a backup's archive", |f| f.contains("/backup/") && f.contains("/holon-archive-") && f.ends_with(".json.gz")),
        ("the policy", |f| f.ends_with("holon.toml")),
    ];
    let listed = || {
        let mut files = Vec::new();
        files_under(&home, &mut files);
        files_under(&backup, &mut files);
        files
    };
    let missing = |files: &[PathBuf]| -> Vec<&str> {
        let seen: Vec<String> = files.iter().map(|f| f.to_string_lossy().replace('\\', "/")).collect();
        kinds.iter().filter(|(_, is)| !seen.iter().any(|f| is(f))).map(|(what, _)| *what).collect()
    };
    let by = Instant::now() + EVENTUALLY;
    while !missing(&listed()).is_empty() {
        assert!(Instant::now() < by, "{:?} not among the files looked at", missing(&listed()));
        std::thread::sleep(Duration::from_millis(200));
    }

    // The control: the canary reached the record, so its absence below means something.
    let export = base.join("export");
    std::fs::create_dir_all(&export).unwrap();
    server.holon(&["export", "kept", "--to", &export.display().to_string()]);
    let mut exported = Vec::new();
    files_under(&export, &mut exported);
    let forms = forms(&canary);
    assert!(exported.iter().any(|f| found_in(&std::fs::read(f).unwrap(), &forms).is_some()), "the export holds the canary: {exported:#?}");
    server.stop();

    // The control again, on what the stopped server left.
    let files = listed();
    let gone = missing(&files);
    assert!(gone.is_empty(), "{gone:?} not among the files looked at: {files:#?}");
    let clear: Vec<String> = files.iter().filter_map(|f| found_in(&std::fs::read(f).unwrap(), &forms).map(|how| format!("{}: {how}", f.display()))).collect();
    assert!(clear.is_empty(), "the canary {canary} in clear, of {} files: {clear:#?}", files.len());
    let said = console();
    assert!(found_in(&said, &forms).is_none(), "the canary {canary} on the server's console: {}", String::from_utf8_lossy(&said));
    // Nor a hash of a state the runs left, which the export carries (`state_after`, in each run's
    // line): the state is small (a counter, the canary), and a plain hash of it lets a guess of it
    // be checked offline.
    let mut hashes = Vec::new();
    for f in exported.iter().filter(|f| f.extension().is_some_and(|e| e == "json")) {
        let text = std::fs::read_to_string(f).unwrap();
        for (at, _) in text.match_indices("state_after") {
            let after = &text[at + "state_after".len()..];
            let from = after.find(|c: char| c.is_ascii_hexdigit()).filter(|n| *n < 8);
            if let Some(hex) = from.map(|n| &after[n..]).and_then(|h| h.get(..64)).filter(|h| h.chars().all(|c| c.is_ascii_hexdigit())) {
                hashes.push(hex.to_ascii_lowercase());
            }
        }
    }
    let hashes: std::collections::HashSet<String> = hashes.into_iter().collect();
    assert!(hashes.len() > 100, "the export's state hashes: {}", hashes.len());
    // Each run of hex digits in a file, each 64 of them in it looked up.
    let hex_hash_in = |bytes: &[u8]| -> Option<String> {
        bytes.split(|b| !b.is_ascii_hexdigit()).filter(|run| run.len() >= 64).find_map(|run| {
            run.windows(64).map(|w| String::from_utf8_lossy(w).to_ascii_lowercase()).find(|w| hashes.contains(w))
        })
    };
    let hashed: Vec<String> = files.iter().filter_map(|f| hex_hash_in(&std::fs::read(f).unwrap()).map(|h| format!("{}: {h}", f.display()))).collect();
    assert!(hashed.is_empty(), "a state's hash in clear: {hashed:#?}");
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
    let said = base.join("serve.err");
    let mut server = Server { child: serve(&home, &root, port, std::fs::File::create(&said).unwrap().into()), dirs: vec![base.clone()], port, home: home.clone() };
    answering(&mut server.child, port, &said);
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
    until("shop answers", 60, || status(&post(shop_port, "/bump", "{}")) == 200);
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
    let by = Instant::now() + EVENTUALLY;
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
    let said = base.join("serve-again.err");
    let mut again = Server { child: serve(&home, &new_root, port, std::fs::File::create(&said).unwrap().into()), dirs: Vec::new(), port, home: home.clone() };
    answering(&mut again.child, port, &said);
    let shown = again.holon(&["key", "status"]);
    assert!(shown.contains("new.key") && shown.contains("d:shop: forgotten"), "{shown}");
    // A root that can't be read any more (a key file on a mount gone): a new deployment is
    // refused at its deploy, saying so, rather than deployed with every run's line lost; and once
    // the root is back, it deploys.
    let new_file = base.join("new.key");
    let away = base.join("new.key.away");
    std::fs::rename(&new_file, &away).unwrap();
    let late = texts.join("late.holon.ts");
    std::fs::write(&late, format!("import {{ flow, route }} from \"holon\"\n\nflow(\"late\", {{ port: {} }})\n\nroute.get(\"/\", () => \"late\")\n", free_port())).unwrap();
    let at = port.to_string();
    let deploy_late = || offline(&home, &["deploy", &late.display().to_string(), "--approve", "--server", &at]);
    let (went, said) = deploy_late();
    assert!(!went && said.contains("keys couldn't be made"), "{said}");
    std::fs::rename(&away, &new_file).unwrap();
    let (went, said) = deploy_late();
    assert!(went, "{said}");
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

/// Guards, against the real server: a deployment's run log that couldn't all be read as the
/// server started (its sealed segments' place a file, not a folder) is an error at the commands
/// that read it all, as at the routes under them (src/tests/unread_readers.rs): `holon runs
/// --last`, `holon export` and `holon restore --to-run … --state --dry-run` each end with a code
/// that isn't 0, say the server's fixed words and no place on its disk, and the export writes no
/// file; `holon host` tells the admin to restart once it reads again. Readable again and
/// restarted, each goes as before.
/// Why: a command that took the server's error as none, or said it and ended with 0, shows a
/// person fewer runs as if they were all, and lets a script go on with an export missing runs
/// (release audit R92).
/// If it fails: the command it names takes a failed answer as empty (`cmdline::json`,
/// `local::admin_call`, verify.rs's `export`), or the route under it answers as if whole again.
#[test]
fn a_run_log_that_cant_all_be_read_is_an_error_at_the_commands() {
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
    let base = std::env::temp_dir().join(format!("holon-unread-{}-{stamp}", std::process::id()));
    let (home, texts, export) = (base.join("home"), base.join("texts"), base.join("export"));
    for d in [&home, &texts, &export] {
        std::fs::create_dir_all(d).unwrap();
    }
    let root = format!("file:{}", base.join("root.key").display());
    let start = |name: &str, dirs: Vec<PathBuf>| {
        let port = free_port();
        let said = base.join(format!("{name}.err"));
        let mut server = Server { child: serve(&home, &root, port, std::fs::File::create(&said).unwrap().into()), dirs, port, home: home.clone() };
        answering(&mut server.child, port, &said);
        server
    };
    let mut first = start("first", vec![base.clone()]);
    let shop_port = free_port();
    let file = texts.join("kept.holon.ts");
    std::fs::write(
        &file,
        format!(
            r#"import {{ flow, route }} from "holon"
import state from "state"

flow("kept", {{ port: {shop_port} }})

route.post("/bump", () => {{
  state.n = (state.n ?? 0) + 1
  return state.n
}})
"#
        ),
    )
    .unwrap();
    first.holon(&["deploy", &file.display().to_string(), "--approve"]);
    until("kept answers", 60, || status(&post(shop_port, "/bump", "{}")) == 200);
    for _ in 0..4 {
        assert_eq!(status(&post(shop_port, "/bump", "{}")), 200);
    }
    let exported = export.join("kept-runs.json");
    let commands = |server: &Server| {
        let (at, to) = (server.port.to_string(), export.display().to_string());
        [("holon runs", vec!["runs", "kept", "--last", "200"]), ("holon export", vec!["export", "kept", "--to", &to]), ("holon restore", vec!["restore", "kept", "--to-run", "1", "--state", "--dry-run"])].map(|(name, mut args)| {
            args.extend(["--server", &at]);
            (name, offline(&home, &args))
        })
    };
    for (name, (went, said)) in commands(&first) {
        assert!(went, "{name}, before: {said}");
    }
    assert!(exported.exists(), "the export, before");
    std::fs::remove_file(&exported).unwrap();
    first.stop();

    // Its sealed segments' place a file: as the log opens, it can't be listed.
    let mut files = Vec::new();
    files_under(&home, &mut files);
    let log = files.iter().find(|f| f.file_name().is_some_and(|n| n == "kept.jsonl")).unwrap_or_else(|| panic!("its run log: {files:#?}"));
    let sealed = log.with_extension("sealed");
    assert!(!sealed.exists(), "five runs sealed nothing");
    std::fs::write(&sealed, b"not a folder").unwrap();
    let mut second = start("second", Vec::new());
    for (name, (went, said)) in commands(&second) {
        assert!(!went, "{name} ends with a code that isn't 0: {said}");
        assert!(said.contains("answered 500: \"kept\"'s run log can't all be read now: the server's warnings say why"), "{name} says the server's fixed words: {said}");
        assert!(!said.contains(&base.display().to_string()) && !said.contains("kept.sealed"), "{name} names no place on the server's disk: {said}");
    }
    assert!(!exported.exists(), "an export that fails writes no file");
    let host = second.holon(&["host"]);
    assert!(host.contains("couldn't be read as the server started") && host.contains("restart the server"), "{host}");
    second.stop();

    // Readable again (nothing there, nothing sealed), and restarted.
    std::fs::remove_file(&sealed).unwrap();
    let mut third = start("third", Vec::new());
    for (name, (went, said)) in commands(&third) {
        assert!(went, "{name}, readable again: {said}");
    }
    assert!(exported.exists(), "the export, readable again");
    let host = third.holon(&["host"]);
    assert!(!host.contains("couldn't be read as the server started"), "{host}");
    third.stop();
}

/// How many low-level writers `text` (a source file) calls outside its test modules, each left
/// out by its braces wherever it sits (source.rs, R78): the scan cut each file at its first one.
fn writers_in(text: &str) -> usize {
    let mut n = 0;
    for (line, _) in text.lines().map(str::trim).zip(source::in_test_modules(text)).filter(|(l, test)| !test && !l.starts_with("//")) {
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
    n
}

/// Each file under `src` that calls low-level writers outside test code, and how many
/// ([`writers_in`]).
fn writers_under(src: &Path) -> std::collections::BTreeMap<String, usize> {
    counted_under(src, writers_in)
}

/// Each file under `src` with calls `count` counts outside test code, and how many: a tests.rs, a
/// file under a tests folder, and a test module's file of its own (`#[cfg(test)] mod x;`,
/// source.rs's test_files) left out.
fn counted_under(src: &Path, count: fn(&str) -> usize) -> std::collections::BTreeMap<String, usize> {
    let mut files = Vec::new();
    files_under(src, &mut files);
    files.retain(|f| f.extension().is_some_and(|e| e == "rs"));
    let tests = source::test_files(&files);
    let mut found = std::collections::BTreeMap::new();
    for f in files.iter().filter(|f| !tests.contains(*f)) {
        let name = f.strip_prefix(src).unwrap().to_string_lossy().replace('\\', "/");
        if name.ends_with("tests.rs") || name.contains("/tests/") || name.starts_with("tests/") {
            continue;
        }
        let n = count(&std::fs::read_to_string(f).unwrap());
        if n > 0 {
            found.insert(name, n);
        }
    }
    found
}

/// Guards: the writers scan reads all of a file but its test modules (R78): one after a test
/// module, or after `mod name;` declaring one, is counted; one in it, or in a string or comment
/// there, isn't, and a brace in a string doesn't end the module early. A test module's file of
/// its own, and what it declares, are left out; a plain module's file is read.
/// Why: the scan cut each file at its first test module, so main.rs, which declares one near its
/// top, was read only to that, near its top, and a writer added below it went unseen.
/// If it fails: source.rs's test_modules misreads where a test module ends.
#[test]
fn the_writers_scan_reads_past_each_test_module() {
    for (text, n) in [
        ("fn f() {\n    std::fs::write(a, b);\n}\n#[cfg(test)]\nmod tests {\n    fn g() {\n        std::fs::write(a, b);\n    }\n}\nfn h() {\n    std::fs::write(a, b);\n}\n", 2),
        ("#[cfg(test)]\nmod surfaces;\nmod sweep;\nfn main() {\n    std::fs::write(a, b);\n}\n", 1),
        ("#[cfg(test)]\n#[allow(dead_code)]\npub(crate) mod in_test {\n    const C: char = '}';\n    const S: &str = \"}}\";\n    // }\n    fn g() { std::fs::write(a, b); }\n}\nfn h() { std::fs::write(a, b); }\n", 1),
        ("mod a {\n    #[cfg(test)]\n    mod tests {\n        fn g() { std::fs::write(a, b); }\n    }\n    fn h() { std::fs::write(a, b); }\n}\n", 1),
        ("#[cfg(test)]\nfn helper() {\n    std::fs::write(a, b);\n}\n", 1),
    ] {
        assert_eq!(writers_in(text), n, "{text}");
    }
    // A test module in a file of its own leaves the count, and what it declares in turn; a plain
    // module's file stays in it.
    let dir = std::env::temp_dir().join(format!("holon-writers-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("t")).unwrap();
    let writes = "fn f() {\n    std::fs::write(a, b);\n}\n";
    for (file, text) in [("lib.rs", "#[cfg(test)]\nmod t;\nmod p;\n".to_string()), ("t.rs", format!("{writes}mod u;\n")), ("t/u.rs", writes.to_string()), ("p.rs", writes.to_string())] {
        std::fs::write(dir.join(file), text).unwrap();
    }
    assert_eq!(writers_under(&dir), std::collections::BTreeMap::from([("p.rs".to_string(), 1)]), "only the plain module's file");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The low-level writers (owner-only, atomic, appending) and the standard library's, outside
/// test code: each file's count of them, and why it may write past at_rest.rs.
const WRITERS: &[(&str, usize, &str)] = &[
    ("at_rest.rs", 8, "where sealing is: it writes what it sealed, keys.lock, empty, and keys.json's copy in the backup folder, by the archiver and as keys.json is written: data keys wrapped under the root"),
    ("bindings.rs", 1, "access.versions.jsonl: lines sealed with seal_line just before"),
    ("blobs.rs", 2, "the file texts' chunks and chunk lists, sealed with seal just before"),
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
    ("storage.rs", 3, "the policy (holon.toml): hand-edited, in clear on purpose (§2.5); and a removed deployment's records moved to the backup, sealed already, byte for byte (R41); and the sealed segments a removal gave up, copied there beside them (R110)"),
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
    let found = writers_under(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
    let allowed: std::collections::BTreeMap<String, usize> = WRITERS.iter().map(|(f, n, _)| (f.to_string(), *n)).collect();
    assert_eq!(found, allowed, "a low-level writer added or gone: seal through at_rest.rs, or say in WRITERS why it may write past it");
}

/// The ways into at_rest.rs that don't ask `data_folder` themselves and touch the disk or a key:
/// each goes through the one named, which does or goes on through another that does (R87).
const THROUGH: &[(&str, &str)] = &[
    ("read_sealed", "read"),
    ("write_table", "write_private"),
    ("write_sealed", "write_private"),
    ("open_export", "Keyring::open_file"),
    ("Keyring::make_scope", "Keyring::current"),
    ("Keyring::rotate", "Keyring::add_version"),
    ("Keyring::add_version", "Keyring::persist"),
    ("Keyring::forget", "Keyring::persist"),
    ("Keyring::rotate_root", "Keyring::rotate_root_until"),
    ("Keyring::rotate_root_until", "Keyring::persist"),
    ("Keyring::seal_file", "Keyring::current"),
    ("Keyring::seal_line", "Keyring::current"),
    ("Keyring::seal_body", "Keyring::current"),
    ("Keyring::open_file", "Keyring::open_file_in"),
    ("Keyring::open_file_in", "Keyring::version"),
    ("Keyring::open_line", "Keyring::version"),
    ("Keyring::open_body", "Keyring::version"),
    ("LineSeal::seal", "Keyring::seal_line"),
    ("LineSeal::seal_body", "Keyring::seal_body"),
    ("LineSeal::seal_lines", "LineSeal::seal"),
    ("LineSeal::seal_file", "Keyring::seal_file"),
    ("LineSeal::read", "Keyring::open_line"),
    ("LineSeal::open", "LineSeal::read"),
    ("LineSeal::open_lines", "LineSeal::open"),
    ("LineSeal::open_file", "Keyring::open_file"),
    ("StoreKeys::seal", "Keyring::seal_file"),
    ("StoreKeys::open", "Keyring::open_file"),
];

/// The ways into at_rest.rs that never run on a request, and why: their errors go to whoever runs
/// the server or the command.
const OUTSIDE: &[(&str, &str)] = &[
    ("install", "the server's keyring, set as it starts"),
    ("lock_folder", "the data folder locked as the server starts, or by holon key rotate --root"),
    ("hold_folder", "the data folder locked as the server starts"),
    ("Keyring::open", "the keyring opened as the server starts, or by a command with no server on the folder"),
    ("Keyring::open_to_read", "the keyring opened by holon verify, a command"),
    ("Keyring::others_on_root", "holon key rotate --root, a command with no server on the folder"),
    ("Keyring::export_root", "holon key export, a command"),
];

/// The ways into at_rest.rs that touch neither the disk nor a key, so nothing in them can be made
/// to fail, and why. Checked: none of them, nor any function of the file they call, reads or
/// writes a file or fetches a key.
const PURE: &[(&[&str], &str)] = &[
    (&["deployment", "deployment_of", "role::state_head", "role::segment", "role::batch", "role::export"], "a scope's or a role's name, made from a slug or a path"),
    (&["seen_held", "key_missing_in", "Unreadable::fmt", "Unreadable::key_missing", "LineSeal::fmt"], "words, or which error one is"),
    (&["StoreKeys::failed"], "a store's error made into what whoever asked may be told (failing::Failed): nothing read or written"),
    (&["note_seen", "seen_forgotten"], "a listener's held record noted in the admin's warnings, or taken back (failing::note_file): nothing read or written"),
    (&["is_sealed", "decimal", "rejoin", "Framed::of", "Framed::line", "Framed::text", "LineSeal::number"], "bytes already in hand, looked at or framed"),
    (&["LineSeal::new", "LineSeal::with", "LineSeal::of", "keys", "store", "Keyring::memory"], "a sealer or a keyring made or handed on, nothing sealed or opened yet (memory: random keys, kept nowhere)"),
    (&["Keyring::key_id", "Keyring::source", "Keyring::made", "Keyring::no_backup", "Keyring::status"], "what the keyring was opened with, held in memory: never a key"),
    (&["Keyring::fingerprint", "Keyring::name", "StoreKeys::name"], "a keyed hash under a key derived as the keyring opened, held in memory: no scope's key fetched"),
];

/// The functions that fetch a key: a scope's (current, version) or the root, read where it is kept.
const KEYS: &[&str] = &["Keyring::current", "Keyring::version", "Keyring::root"];

/// A function of at_rest.rs outside its test modules: its name (`Type::name` in an impl,
/// `module::name` in a module), the text of its body (from the line that opens it), whether it is
/// a way in (`pub(crate)`, or a trait's method), and whether a cfg gates it (the fault's own: the
/// tests' or its stand-in in every other build).
struct AtRestFn {
    name: String,
    body: String,
    way_in: bool,
    gated: bool,
}

/// Each function of at_rest.rs outside its test modules, and the names of its modules.
fn at_rest_fns(text: &str) -> (Vec<AtRestFn>, std::collections::BTreeSet<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let test = source::in_test_modules(text);
    let (mut found, mut mods) = (Vec::new(), std::collections::BTreeSet::new());
    let (mut of, mut trait_impl, mut i) = (None::<String>, false, 0);
    while i < lines.len() {
        let line = lines[i];
        if test[i] {
            i += 1;
            continue;
        }
        if let Some(head) = line.strip_prefix("impl").filter(|h| h.starts_with(' ') || h.starts_with('<')) {
            trait_impl = head.contains(" for ");
            let head = head.split(" for ").last().unwrap_or(head);
            of = head.trim().trim_end_matches('{').trim().split('<').next().map(|t| t.trim().to_string());
        } else if let Some(module) = ["pub(crate) mod ", "pub mod ", "mod "].iter().find_map(|p| line.strip_prefix(p)).and_then(|m| m.strip_suffix(" {")) {
            (of, trait_impl) = (Some(module.to_string()), false);
            mods.insert(module.to_string());
        } else if line == "}" {
            (of, trait_impl) = (None, false);
        }
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        // pub(crate) in the server; pub in holon-host, a library.
        let public = trimmed.starts_with("pub(crate) fn ") || trimmed.starts_with("pub fn ");
        if public || trimmed.starts_with("fn ") {
            let name = trimmed.split("fn ").nth(1).unwrap().split(['(', '<']).next().unwrap().to_string();
            let name = match (&of, indent) {
                (Some(of), 4) => format!("{of}::{name}"),
                _ => name,
            };
            let gated = lines[..i].iter().rev().map(|l| l.trim()).take_while(|l| l.starts_with("#[") || l.starts_with("///")).any(|l| l.starts_with("#[cfg("));
            // To the line that opens its body: or a body on one line, opened and closed there.
            let one_line = |l: &str| l.contains('{') && l.trim_end().ends_with('}');
            let mut j = i;
            while j + 1 < lines.len() && !lines[j].trim_end().ends_with('{') && !lines[j].trim_end().ends_with(';') && !one_line(lines[j]) {
                j += 1;
            }
            // A trait's declaration, with no body: nothing to read.
            if lines[j].trim_end().ends_with(';') {
                i = j + 1;
                continue;
            }
            let end = format!("{}}}", " ".repeat(indent));
            let close = if one_line(lines[j]) { j } else { (j + 1..lines.len()).find(|&k| lines[k] == end).unwrap_or(lines.len() - 1) };
            found.push(AtRestFn { name, body: lines[j..=close].join("\n"), way_in: public || (trait_impl && indent == 4), gated });
            i = close + 1;
            continue;
        }
        i += 1;
    }
    (found, mods)
}

fn ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `body` calls the function of at_rest.rs named `name`: a top-level one bare, a module's
/// as `module::f`, a type's method as `.m(`, `Type::m(` or `Self::m(` (any type's `.m(`: more
/// calls than there are, never fewer).
fn calls(body: &str, name: &str, mods: &std::collections::BTreeSet<String>) -> bool {
    let (of, short) = name.rsplit_once("::").map_or((None, name), |(o, s)| (Some(o), s));
    let called = format!("{short}(");
    let as_path = |before: &str, path: &str| before.strip_suffix("::").and_then(|b| b.strip_suffix(path)).is_some_and(|b| !b.ends_with(ident));
    body.match_indices(&called).any(|(at, _)| {
        let (before, after) = (&body[..at], &body[at + called.len()..]);
        if before.ends_with(ident) {
            return false;
        }
        match of {
            None => !before.ends_with('.') && !before.ends_with("::"),
            Some(module) if mods.contains(module) => as_path(before, module),
            // A lock's read() or write() isn't the method of that name.
            Some(_) if ["read", "write", "lock"].contains(&short) && after.starts_with(')') => false,
            Some(of) => before.ends_with('.') || as_path(before, of) || as_path(before, "Self"),
        }
    })
}

/// Whether `body` touches the disk itself: a file read or written, a root read or kept where it
/// is, or the hook asked.
fn touches_disk(body: &str) -> bool {
    let bare = |what: &str| body.match_indices(what).any(|(at, _)| !body[..at].ends_with(ident));
    bare("fs::")
        || bare("File::")
        || bare("disk::")
        || ["OpenOptions", "crate::sealing::", "data_folder(", "source.read(", "source.create(", "source.keep(", ".keyrings(", ".keep_keyrings("].iter().any(|w| body.contains(w))
}

/// Guards: every way into at_rest.rs, a `pub(crate)` function or a trait's method, Result or not,
/// can be made to fail where it touches the disk or a key: it asks `data_folder` itself (or reads
/// through `disk`, which does), or goes through one that does (THROUGH), or never runs on a
/// request (OUTSIDE, with why), or touches neither (PURE, with why, and checked: nothing it calls
/// in the file does). So the data folder made to fail reaches everything a request does there,
/// and `no_answer_says_where_or_why_the_data_folder_failed` sees what each answer says then
/// (audit R87).
/// Why: answers passed on where a server's file failed and why (R71, R85, R91), each found by
/// reading; the sweep sees only the ways in that ask the fault, and one added that doesn't would
/// hide every answer behind it. A reader that swallows its error (a line log read to its lines)
/// and the store's sealing (a trait's methods) are ways in like the rest.
/// If it fails: a new way into at_rest.rs asks `data_folder(Some(path))` before it touches the
/// disk (`data_folder(None)` for a key), or is named in THROUGH with what it goes through, in
/// OUTSIDE with why it never runs on a request, or in PURE with why it touches neither.
#[test]
fn every_way_into_at_rest_can_be_made_to_fail() {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/at_rest.rs")).unwrap();
    let (fns, mods) = at_rest_fns(&text);
    let fns: Vec<&AtRestFn> = fns.iter().filter(|f| !f.gated).collect();
    let named: std::collections::BTreeMap<&str, &str> = fns.iter().map(|f| (f.name.as_str(), f.body.as_str())).collect();
    let asks = |name: &str| named.get(name).is_some_and(|body| body.contains("data_folder(") || body.contains("disk::"));
    let pure: std::collections::BTreeMap<&str, &str> = PURE.iter().flat_map(|(names, why)| names.iter().map(move |n| (*n, *why))).collect();
    let mut wrong = Vec::new();
    for (name, through) in THROUGH {
        let Some(body) = named.get(name) else {
            wrong.push(format!("{name}: in THROUGH, and no function of at_rest.rs"));
            continue;
        };
        if !calls(body, through, &mods) {
            wrong.push(format!("{name}: said to go through {through}, which it doesn't call"));
        }
    }
    // Through, to one that asks: never round in a ring.
    let reaches = |name: &str| {
        let mut at = name.to_string();
        for _ in 0..THROUGH.len() + 1 {
            if asks(&at) {
                return true;
            }
            match THROUGH.iter().find(|(n, _)| *n == at) {
                Some((_, next)) => at = next.to_string(),
                None => return false,
            }
        }
        false
    };
    // What touches the disk or a key: itself, or through any function of the file it calls.
    let mut touch: std::collections::BTreeSet<&str> = fns.iter().filter(|f| touches_disk(&f.body) || KEYS.contains(&f.name.as_str())).map(|f| f.name.as_str()).collect();
    loop {
        let more: Vec<&str> = fns
            .iter()
            .filter(|f| !touch.contains(f.name.as_str()) && touch.iter().any(|t| *t != f.name && calls(&f.body, t, &mods)))
            .map(|f| f.name.as_str())
            .collect();
        if more.is_empty() {
            break;
        }
        touch.extend(more);
    }
    for f in fns.iter().filter(|f| f.way_in) {
        let n = f.name.as_str();
        let classes: Vec<&str> = [
            ("asks", asks(n)),
            ("THROUGH", THROUGH.iter().any(|(t, _)| *t == n)),
            ("OUTSIDE", OUTSIDE.iter().any(|(o, _)| *o == n)),
            ("PURE", pure.contains_key(n)),
        ]
        .into_iter()
        .filter(|(_, is)| *is)
        .map(|(class, _)| class)
        .collect();
        match classes.as_slice() {
            [] => wrong.push(format!("{n}: a way in of no class (touches the disk or a key: {})", touch.contains(n))),
            ["THROUGH"] if !reaches(n) => wrong.push(format!("{n}: goes through no function that asks data_folder")),
            ["PURE"] if touch.contains(n) => {
                let through: Vec<&&str> = touch.iter().filter(|t| **t != n && calls(&f.body, t, &mods)).collect();
                wrong.push(format!("{n}: in PURE, and touches the disk or a key (through {through:?})"));
            }
            [_] => {}
            _ => wrong.push(format!("{n}: in more than one class: {classes:?}")),
        }
    }
    for name in OUTSIDE.iter().map(|(n, _)| *n).chain(pure.keys().copied()) {
        if !named.contains_key(name) {
            wrong.push(format!("{name}: listed, and no function of at_rest.rs"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // The scan reads what it should: the ways that ask, the ones that swallow their error, the
    // store's sealing, and the fault's own left out.
    for name in ["Keyring::persist", "Keyring::current", "Keyring::version", "write_private", "read", "read_sealed_in", "LineSeal::append", "Keyring::keep_copy_in", "LineSeal::read_lines", "disk::read_dir"] {
        assert!(asks(name), "{name} asks data_folder: the scan doesn't read it");
    }
    for name in ["StoreKeys::seal", "StoreKeys::open", "LineSeal::open", "LineSeal::read_lines", "disk::open"] {
        assert!(fns.iter().any(|f| f.name == name && f.way_in), "{name} is a way in: the scan doesn't see it");
    }
    assert!(!named.contains_key("fail_folder") && !named.contains_key("failing_folder"), "the fault's own, gated by cfg, are left out");
    assert!(touch.contains("Keyring::open_line") && !touch.contains("Keyring::name"), "{touch:?}");
    assert!(fns.iter().filter(|f| f.way_in).count() > 60, "the scan finds at_rest.rs's ways in");
}

/// Guards: the call scan tells a call from what only looks like one (R87): a lock's `read()`
/// isn't `LineSeal::read`, `Vec::new(` isn't `LineSeal::new`, `KeysFile::` isn't a file, and a
/// longer name holding a short one isn't it; a call made any of the ways Rust makes one is seen.
/// If it fails: `calls` or `touches_disk` misreads a body, and the guard above with it.
#[test]
fn the_at_rest_scan_tells_a_call_from_a_name() {
    let mods: std::collections::BTreeSet<String> = ["disk".to_string()].into();
    for (body, name, called) in [
        ("self.inner.read().unwrap()", "LineSeal::read", false),
        ("self.read(line).ok()", "LineSeal::read", true),
        ("Vec::new()", "LineSeal::new", false),
        ("LineSeal::new(&deployment(slug), role)", "LineSeal::new", true),
        ("Self::open_as(path, None, &[], true)", "Keyring::open_as", true),
        ("crate::paths::canonical(path)", "Keyring::canonical", false),
        ("std::fs::read(path)", "disk::read", false),
        ("disk::read(path)", "disk::read", true),
        ("read_sealed_in(path, scope, role)", "read", false),
        ("keys.read_sealed(path)", "read_sealed", false),
        ("match read(keys, path, role) {", "read", true),
    ] {
        assert_eq!(calls(body, name, &mods), called, "{body} calls {name}");
    }
    for (body, touches) in [("KeysFile::default()", false), ("std::fs::File::open(p)", true), ("let fs_used = 1;", false), ("source.keyrings()?", true), ("inner.source.clone()", false)] {
        assert_eq!(touches_disk(body), touches, "{body}");
    }
}

/// How many low-level reads `text` (a source file) makes outside its test modules: a file read
/// whole, opened, or a folder listed, past at_rest::disk.
fn readers_in(text: &str) -> usize {
    text.lines()
        .map(str::trim)
        .zip(source::in_test_modules(text))
        .filter(|(l, test)| !test && !l.starts_with("//"))
        .map(|(line, _)| ["fs::read(", "fs::read_to_string(", "File::open(", "fs::read_dir(", ".read_dir()"].iter().map(|r| line.matches(*r).count()).sum::<usize>())
        .sum()
}

/// The low-level reads left past at_rest::disk, per file, and why each may read past the hook:
/// what it reads isn't the data folder, or no request, run or listener reads it (R87).
const READERS: &[(&str, usize, &str)] = &[
    ("at_rest.rs", 7, "the hook's own readers (disk's four), and the keyring opened as the server starts, keys files beside the root, a table written in clear before sealing: at start or by a rotation"),
    ("bounds.rs", 1, "the container's memory limit, in its cgroup files: not the data folder"),
    ("cmdline.rs", 1, "the command line reading a file a person named"),
    ("deploycmd.rs", 1, "holon deploy reading the flow's text a person named"),
    ("editor.rs", 1, "the editor's own files, from a folder given as the server starts: not the data folder"),
    ("flowcmd.rs", 2, "the command line reading a flow's text, and its types file, where a person named"),
    ("flowscmd.rs", 1, "holon flows save reading the text a person named"),
    ("folders.rs", 3, "a bound folder: the person's own, read by the flow it is bound to, not the server's"),
    ("keys.rs", 1, "holon key export: keys.json's id read by the command"),
    ("local.rs", 3, "the command line's own files: where a server is, its sign-in, runs kept"),
    ("mcphosts.rs", 5, "an MCP client's config file, where a person asked"),
    ("migrate.rs", 4, "holon migrate: key files an older release kept in the config folder"),
    ("pick.rs", 1, "the folder picker listing the server's disk for a folder to bind, at an admin's asking: what it lists is its answer"),
    ("replaycmd.rs", 1, "holon replay reading the flow's text a person named"),
    ("sealing.rs", 3, "the root key's own file and the keys files listed beside it, kept apart from the data folder; an old release's secrets file, at uninstall"),
    ("secret_sources.rs", 1, "the env file an admin points the secrets at: theirs, outside the data folder"),
    ("selfmanage.rs", 3, "holon's own install, read on update and uninstall"),
    ("storage.rs", 1, "the process's own memory (/proc/self/status): not the data folder"),
    ("verify.rs", 4, "holon verify and export: an export, and what a rerun read, where a person named"),
    ("workbench/app.rs", 2, "the terminal app reading a file a person opened"),
    ("workbench/shell.rs", 1, "the machine's name (/etc/hostname)"),
];

/// Guards: nothing under the data folder is read past at_rest.rs's hook but the readers named,
/// each with why (R87): every other read of a file or a listing goes through `at_rest::disk`,
/// which asks `data_folder` first, so the data folder made to fail reaches it.
/// Why: the sweep fails only what asks the hook; a read added later past it, on a request's path,
/// would answer from a disk that never fails in the tests, and what it passes on when a real one
/// does would go unseen.
/// If it fails: read through `crate::at_rest::disk` (the same call, after the check), or, if what
/// it reads isn't the data folder or no request, run or listener reads it, add it to READERS with
/// why.
#[test]
fn no_reader_under_the_data_folder_bypasses_the_hook() {
    let found = counted_under(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), readers_in);
    let allowed: std::collections::BTreeMap<String, usize> = READERS.iter().map(|(f, n, _)| (f.to_string(), *n)).collect();
    assert_eq!(found, allowed, "a low-level reader added or gone: read through at_rest::disk, or say in READERS why it may read past it");
    assert_eq!(readers_in("let a = std::fs::read(p);\nlet b = crate::at_rest::disk::read(p);\n// std::fs::read(q)\nfor e in dir.read_dir()? {}\n"), 2);
}

/// holon-host's functions that read a file or list a folder past the store's hook, and why: the
/// rest ask it (`at_rest.data_folder(path)`) before they read (R87).
const HOST_OUTSIDE: &[(&str, &str, &str)] = &[
    ("lib.rs", "sync_dir", "a folder opened to sync its names to disk after a write: nothing of it read"),
    ("packs.rs", "Packs::open", "the packs listed once, as the store opens (the server's start, or the store sweep's first look): never on a request; failed then, the store would go without its packs for the rest of the process"),
    ("packs.rs", "read_index", "a pack's index, read once as the store opens, with the listing"),
];

/// Guards: holon-host's store asks the data folder's hook (AtRest::data_folder) before each read
/// of a file of its own on a request's path, so the server's tests fail it with the rest (R87):
/// every function there that reads a file or lists a folder asks it, or is named in HOST_OUTSIDE
/// with why.
/// Why: the store is a crate below the server, its reads its own; one added past the hook would
/// read a disk that never fails in the tests.
/// If it fails: ask `self.at_rest.data_folder(&path)` before the read, or name the function in
/// HOST_OUTSIDE with why no request reaches it.
#[test]
fn every_read_of_the_store_asks_the_hook() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../holon-host/src");
    let mut wrong = Vec::new();
    let mut asked = 0;
    for file in ["lib.rs", "packs.rs"] {
        let (fns, _) = at_rest_fns(&std::fs::read_to_string(src.join(file)).unwrap());
        for f in fns.iter().filter(|f| readers_in(&f.body) > 0) {
            let outside = HOST_OUTSIDE.iter().any(|(at, name, _)| *at == file && *name == f.name);
            if f.body.contains(".data_folder(") {
                asked += 1;
            } else if !outside {
                wrong.push(format!("{file}: {} reads past the store's hook", f.name));
            }
        }
        for (_, name, _) in HOST_OUTSIDE.iter().filter(|(at, _, _)| *at == file) {
            if !fns.iter().any(|f| f.name == *name && readers_in(&f.body) > 0) {
                wrong.push(format!("{file}: {name} is in HOST_OUTSIDE, and reads nothing"));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(asked >= 3, "get, get_value and a pack's block ask the hook: the scan finds {asked}");
}

/// The cfg attributes above each definition of `item` in `text` (a fn, const, static, or a
/// `thread_local!` holding it), one string per definition.
fn gates_of(text: &str, item: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let defines = |line: &str| {
        let t = line.trim_start().trim_start_matches("pub(crate) ").trim_start_matches("pub(super) ");
        !t.contains("(&self") && ["fn ", "const ", "static ", "struct "].iter().any(|k| t.strip_prefix(k).is_some_and(|r| r.strip_prefix(item).is_some_and(|r| r.starts_with(['(', ':', '<', ' ']))))
    };
    (0..lines.len())
        .filter(|&i| defines(lines[i]))
        .map(|i| {
            // A static in a thread_local! is gated where the macro is.
            let at = if lines[i].trim_start().starts_with("static ") && i > 0 && lines[..i].iter().rev().find(|l| !l.trim().starts_with("///")).is_some_and(|l| l.trim() == "thread_local! {") {
                lines[..i].iter().rposition(|l| l.trim() == "thread_local! {").unwrap()
            } else {
                i
            };
            lines[..at].iter().rev().map(|l| l.trim()).take_while(|l| l.starts_with("#[") || l.starts_with("///")).filter(|l| l.starts_with("#[cfg")).collect::<Vec<_>>().join(" ")
        })
        .collect()
}

/// The body of the definition of fn `item` in `text` gated by `gate`, on one line, trimmed.
fn body_under(text: &str, item: &str, gate: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let at = (0..lines.len())
        .find(|&i| {
            let t = lines[i].trim_start().trim_start_matches("pub(crate) ").trim_start_matches("pub(super) ");
            t.strip_prefix("fn ").is_some_and(|r| r.strip_prefix(item).is_some_and(|r| r.starts_with('(')))
                && lines[..i].iter().rev().map(|l| l.trim()).take_while(|l| l.starts_with("#[") || l.starts_with("///")).any(|l| l == gate)
        })
        .unwrap_or_else(|| panic!("{item} under {gate}: no such definition"));
    let indent = lines[at].len() - lines[at].trim_start().len();
    let end = format!("{}}}", " ".repeat(indent));
    let close = (at + 1..lines.len()).find(|&k| lines[k] == end).unwrap();
    lines[at + 1..close].iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ")
}

/// Guards: the data folder fails in the tests alone (R87). What makes it fail (the folders made
/// to fail, the error and its words, the per-thread faults, the keyring's writes made to fail) is
/// `#[cfg(test)]`, gated by no other cfg or feature; outside the tests `data_folder` is `Ok(())`,
/// crate::faults' `failing` is `None` and the keyring's `write` is `Ok(())`, each inlined. So a
/// server built for anything but its tests has nothing to set, no error to make and no text saying
/// one; the e2e setup holds the built binary to not holding the words.
/// Why: the hook sits in every read and write of the data folder; a fault reachable outside the
/// tests would be a way to fail a real server's disk.
/// If it fails: gate the item named by `#[cfg(test)]` alone, with its stand-in under
/// `#[cfg(not(test))]` doing nothing that fails.
#[test]
fn the_data_folder_fails_in_tests_alone() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let at_rest = std::fs::read_to_string(src.join("at_rest.rs")).unwrap();
    let faults = std::fs::read_to_string(src.join("faults.rs")).unwrap();
    let test = "#[cfg(test)]";
    let mut wrong = Vec::new();
    for (file, text, items) in [
        ("at_rest.rs", &at_rest, &["FAILING", "MADE_TO_FAIL", "FAILING_FOLDERS", "fail_folder", "heal_folder", "failing_folder", "FAILING_SITES", "fail_sites", "heal_sites", "Fired", "FIRED", "fired", "fail_at", "AT"][..]),
        ("faults.rs", &faults, &["FAILING", "fail", "heal"][..]),
    ] {
        for item in items {
            let gates = gates_of(text, item);
            if gates.is_empty() || gates.iter().any(|g| g != test) {
                wrong.push(format!("{file}: {item} is gated by {gates:?}, not #[cfg(test)] alone"));
            }
        }
    }
    for (file, text, item, stand_in) in [("at_rest.rs", &at_rest, "data_folder", "Ok(())"), ("at_rest.rs", &at_rest, "write", "Ok(())"), ("faults.rs", &faults, "failing", "None")] {
        let gates = gates_of(text, item);
        if gates != [test, "#[cfg(not(test))]"] {
            wrong.push(format!("{file}: {item} is gated by {gates:?}, not once by #[cfg(test)] and once by #[cfg(not(test))]"));
        } else if body_under(text, item, "#[cfg(not(test))]") != stand_in {
            wrong.push(format!("{file}: {item} outside the tests does more than {stand_in}"));
        }
    }
    for file in ["at_rest.rs", "faults.rs"] {
        let text = std::fs::read_to_string(src.join(file)).unwrap();
        if text.contains("cfg(feature") || text.contains("cfg(debug_assertions") {
            wrong.push(format!("{file}: a cfg other than test gates something in it"));
        }
    }
    // The store's hook (holon-host's AtRest::data_folder) is defined twice outside the tests: its
    // default, which never fails, and StoreKeys's, which asks the one above.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut defined = Vec::new();
    for krate in ["holon", "holon-host", "holon-core", "holon-author", "holon-module", "holon-wasm"] {
        let src = crates.join(krate).join("src");
        let mut files = Vec::new();
        files_under(&src, &mut files);
        files.retain(|f| f.extension().is_some_and(|e| e == "rs"));
        let tests = source::test_files(&files);
        for f in files.iter().filter(|f| !tests.contains(*f)) {
            let name = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            if name.ends_with("tests.rs") || name.starts_with("tests/") || name.contains("/tests/") {
                continue;
            }
            let text = std::fs::read_to_string(f).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            for (i, test) in source::in_test_modules(&text).into_iter().enumerate() {
                if !test && lines[i].trim_start().starts_with("fn data_folder(&self") {
                    defined.push(format!("{krate}/{name}: {}", lines.get(i + 1).map_or("", |l| l.trim())));
                }
            }
        }
    }
    if defined != ["holon/at_rest.rs: data_folder(Some(path))", "holon-host/lib.rs: Ok(())"] {
        wrong.push(format!("the store's hook is defined other than by its default (Ok(())) and StoreKeys's (at_rest's data_folder): {defined:?}"));
    }
    // No feature of the workspace names the faults: none can turn one on.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut manifests = vec![root.join("Cargo.toml")];
    manifests.extend(std::fs::read_dir(root.join("crates")).unwrap().flatten().map(|e| e.path().join("Cargo.toml")).filter(|p| p.exists()));
    for manifest in manifests {
        let text = std::fs::read_to_string(&manifest).unwrap();
        let features = text.split("\n[").find(|s| s.starts_with("features]")).unwrap_or_default();
        if features.lines().filter(|l| !l.trim_start().starts_with('#')).any(|l| l.to_lowercase().contains("fault") || l.to_lowercase().contains("fail")) {
            wrong.push(format!("{}: a feature names the faults", manifest.display()));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The functions of the server that read why one of its files failed (`Failed::for_the_admin`:
/// where it is on the server's disk and the error's own text), each with where it sends it: none
/// builds an answer with it (audit R87).
const FOR_THE_ADMIN: &[(&str, &str)] = &[
    ("main.rs: serve", "the policy file that doesn't load as the server starts: refuse_to_start says why on stderr, and nothing answers"),
    ("at_rest.rs: read_seen", "disk's own error (a Failed already) made the why of the Failed it gives, whose Display says only what to do"),
    ("at_rest.rs: note_seen", "the admin's warnings (failing::note_file), the record's path the warning's place"),
    ("recorder.rs: noted", "the admin's warnings (failing::note_file, RUNS_UNREAD), the run log's path the warning's place; whoever read it is answered recorder::runs_unread's fixed words"),
    ("recorder.rs: LogCell::kept_runs", "a spooled mail's or message's warning as the server starts (spools.rs: the admin's warnings, and stderr), told and why as the log's own warnings say them; no answer is built from it"),
];

/// Guards: why a file of the server's failed (`Failed::for_the_admin`) is read only where
/// FOR_THE_ADMIN says: every call of it in the server's sources, outside the tests, is in a
/// function it lists (by file and function, source.rs's `fn_spans`), and each it lists still calls
/// it (audit R87).
/// Why: a Failed's Display and Debug say only what whoever asked is told, so a route that formats
/// one can't pass where or why on; one that read why by name could, and only reading would find it.
/// If it fails: send why to stderr or the admin's warnings (`Failed::said`, `failing::note_file`)
/// and answer with what Display says; a function that only does so goes in FOR_THE_ADMIN, saying
/// where it sends it.
#[test]
fn only_what_is_for_the_admin_reads_why_a_file_failed() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    files_under(&src, &mut files);
    files.retain(|f| f.extension().is_some_and(|e| e == "rs"));
    let tests = source::test_files(&files);
    let mut found = std::collections::BTreeSet::new();
    for f in files.iter().filter(|f| !tests.contains(*f)) {
        let name = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        if name.ends_with("tests.rs") || name.contains("/tests/") || name.starts_with("tests/") {
            continue;
        }
        let code = source::inert(&std::fs::read_to_string(f).unwrap());
        let spans = source::fn_spans(&code);
        for (n, (line, test)) in code.lines().zip(source::test_modules(&code)).enumerate() {
            if !test && line.contains("for_the_admin(") && !line.contains("fn for_the_admin(") {
                found.insert(format!("{name}: {}", source::fn_at(&spans, n).unwrap_or("(no function)")));
            }
        }
    }
    let listed: std::collections::BTreeSet<String> = FOR_THE_ADMIN.iter().map(|(f, _)| f.to_string()).collect();
    assert_eq!(found, listed, "why a file failed read where FOR_THE_ADMIN doesn't say, or listed and read no more");
    // The scan names a call by the function it is in, a method by its type.
    let code = "impl Failed {\n    fn why(&self) -> &str {\n        self.for_the_admin()\n    }\n}\nfn f() {}\n";
    let spans = source::fn_spans(code);
    assert_eq!((source::fn_at(&spans, 2), source::fn_at(&spans, 5)), (Some("Failed::why"), Some("f")));
}

/// Guards, against the real server: a new server says, as it starts, beside its setup code, that
/// no one has chosen where the runs its logs let go of go, and what to do; `holon host` (its
/// warning and its line) and `holon policy` say "not chosen"; another setting written keeps it
/// so; `holon policy --backup none` ends the warning (said in its log at the next look), and a
/// folder after it keeps it ended; the line taken out by hand and the policy reloaded, it is not
/// chosen again; started again, the server reads the choice from its file, and doesn't warn.
/// Why: a new server deleted the runs its logs let go of for good without asking anyone, and an
/// absent `backup` read as `none` (R123).
/// If it fails: permissions.rs `Backup`, storage.rs `warnings` or `set_storage`, or the commands'
/// lines (storage.rs `host_lines`, grantcmd.rs `policy_lines`) tell a state as another.
#[test]
fn a_new_server_warns_until_an_admin_chooses_where_runs_go() {
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
    let base = std::env::temp_dir().join(format!("holon-backup-choice-{}-{stamp}", std::process::id()));
    let (home, backup) = (base.join("home"), base.join("backup"));
    std::fs::create_dir_all(&home).unwrap();
    let root = format!("file:{}", base.join("root.key").display());
    let start = |port: u16, n: u32| {
        Command::new(HOLON)
            .args(["serve", "--at", &format!("127.0.0.1:{port}"), "--root-key", &root])
            .env("HOLON_HOME", &home)
            .env_remove("HOLON_DATA")
            .env_remove("HOLON_RUN_DIR")
            .stdout(std::fs::File::create(base.join(format!("console{n}.out"))).unwrap())
            .stderr(std::fs::File::create(base.join(format!("console{n}.err"))).unwrap())
            .spawn()
            .expect("holon serve starts")
    };
    let console = |n: u32| ["out", "err"].map(|f| std::fs::read_to_string(base.join(format!("console{n}.{f}"))).unwrap_or_default()).concat();
    let until = |n: u32, words: &str| {
        let by = Instant::now() + EVENTUALLY;
        while !console(n).contains(words) {
            assert!(Instant::now() < by, "{words:?} in the server's log: {}", console(n));
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    const WARNED: &str = "holon: warning: no backup folder is chosen: the runs each flow's log lets go of (past its keep, or its age) are deleted for good; choose a folder";
    let port = free_port();
    let mut server = Server { child: start(port, 1), dirs: vec![base.clone()], port, home: home.clone() };
    answering(&mut server.child, port, &base.join("console1.err"));
    until(1, WARNED);
    let said = console(1);
    assert!(said.contains("no accounts yet. Open the editor and set up the first admin with this code"), "beside its setup code: {said}");
    assert_eq!(said.matches(WARNED).count(), 1, "said once: {said}");

    let line = |server: &Server, args: &[&str]| server.holon(args).lines().find(|l| l.starts_with("backup ")).map(String::from).unwrap_or_default();
    let warns = |server: &Server| server.holon(&["host"]).contains("! no backup folder is chosen");
    assert!(warns(&server), "holon host shows the warning first");
    assert!(line(&server, &["host"]).contains("not chosen: the runs a log lets go of are deleted for good"), "{}", line(&server, &["host"]));
    assert!(line(&server, &["policy"]).contains("not chosen"), "{}", line(&server, &["policy"]));

    // Another setting written: still not chosen, and no backup line in the file.
    server.holon(&["policy", "--sweep-every", "1h"]);
    let mut files = Vec::new();
    files_under(&home, &mut files);
    let policy = files.into_iter().find(|f| f.file_name().is_some_and(|n| n == "holon.toml")).expect("the policy file, written");
    assert!(!std::fs::read_to_string(&policy).unwrap().contains("backup"), "no backup line written");
    assert!(warns(&server) && line(&server, &["host"]).contains("not chosen"), "another setting keeps it not chosen");

    // None, on purpose: no longer warned of, and the log says so at the next look.
    server.holon(&["policy", "--backup", "none"]);
    assert!(!warns(&server), "chosen none: no warning");
    assert!(line(&server, &["host"]).contains("none, chosen"), "{}", line(&server, &["host"]));
    until(1, "holon: no longer warning of backup-choice");
    // A folder: chosen too.
    server.holon(&["policy", "--backup", &backup.display().to_string()]);
    assert!(!warns(&server) && line(&server, &["policy"]).contains(&backup.display().to_string()), "{}", line(&server, &["policy"]));

    // The line taken out by hand, the policy reloaded: not chosen again.
    let text: String = std::fs::read_to_string(&policy).unwrap().lines().filter(|l| !l.trim_start().starts_with("backup")).map(|l| format!("{l}\n")).collect();
    std::fs::write(&policy, text).unwrap();
    server.holon(&["policy", "--reload"]);
    assert!(warns(&server) && line(&server, &["host"]).contains("not chosen"), "taken out by hand: not chosen again");

    // Chosen, and started again: read from its file, and no warning.
    server.holon(&["policy", "--backup", "none"]);
    server.stop();
    let port = free_port();
    let mut again = Server { child: start(port, 2), dirs: Vec::new(), port, home: home.clone() };
    answering(&mut again.child, port, &base.join("console2.err"));
    assert!(!warns(&again) && line(&again, &["host"]).contains("none, chosen"), "started again: {}", line(&again, &["host"]));
    again.stop();
    assert!(!console(2).contains("no backup folder is chosen"), "nothing said of it as it started again: {}", console(2));
}
