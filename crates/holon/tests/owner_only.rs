//! What the server writes is its owner's alone, against the real server: `holon serve` started as
//! it is installed, a flow deployed with `holon deploy` that leaves something in every kind of
//! file a run leaves, requests sent over sockets, then every file under its home looked at
//! (docs/L4s/at-rest-encryption-L4.md §1.2, step 0), and what it keeps sealed: every table
//! (step 2), every line log (step 3) and the content store (step 4). A test binary of its own, as
//! data_plane.rs.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The server's home: stopped and taken away when dropped.
struct Server {
    child: Child,
    home: PathBuf,
    outside: PathBuf,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
        let _ = std::fs::remove_dir_all(&self.outside);
    }
}

impl Server {
    /// `holon <args>` as this server's own command line: its home, its port.
    fn holon(&self, args: &[&str]) {
        let out = Command::new(HOLON).args(args).env("HOLON_HOME", &self.home).env_remove("HOLON_DATA").env_remove("HOLON_RUN_DIR").output().expect("holon runs");
        assert!(out.status.success(), "holon {args:?}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
}

/// Guards: every file the server writes under its home (the data folder, its keys, the command
/// line's own files) is readable by its owner only: `0o600`, or on Windows a protected DACL
/// granting its owner and SYSTEM, nothing inherited from the folder.
/// Why: the folder's own permissions are the user's (a home folder others may list, a share);
/// on Windows a file made with plain `OpenOptions` or `fs::write` takes the folder's ACL, so one
/// writer that forgets leaves what it writes readable by whoever may read the folder.
/// If it fails: the file it names was written past `sealing::write_private`, `write_aside`,
/// `append_private` or `holon_host::write_atomically`, or one of those stopped restricting it.
#[test]
fn every_file_the_server_writes_is_its_owners_alone() {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = std::env::temp_dir().join(format!("holon-owner-only-{}-{stamp}", std::process::id()));
    // The test's own files (the flow's text) beside it, not in it: they are not the server's.
    let outside = std::env::temp_dir().join(format!("holon-owner-only-texts-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let port = free_port();
    let child = Command::new(HOLON)
        .args(["serve", "--at", &format!("127.0.0.1:{port}"), "--root-key", &format!("file:{}", home.join("root.key").display())])
        .env("HOLON_HOME", &home)
        .env_remove("HOLON_DATA")
        .env_remove("HOLON_RUN_DIR")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("holon serve starts");
    let server = Server { child, home: home.clone(), outside: outside.clone(), port };
    let until = Instant::now() + Duration::from_secs(60);
    while TcpStream::connect(("127.0.0.1", server.port)).is_err() {
        assert!(Instant::now() < until, "holon serve answers on {}", server.port);
        std::thread::sleep(Duration::from_millis(100));
    }

    // State, a collection, console lines, a waiting run and its event; runs enough that the log
    // seals its older ones into a segment.
    let flow_port = free_port();
    let text = format!(
        r#"import {{ action, event, flow, route }} from "holon"
import state from "state"
import notes from "collection:notes"

flow("kept", {{ port: {flow_port} }})

route.post("/note", (req) => {{
  console.log("noted", req.json.text)
  state.last = req.json.text
  notes[req.json.text] = {{ text: req.json.text }}
  const started = waits(req.json)
  return `/done/${{started.waiting.done}}`
}})

route.post("/bump", () => {{
  state.n = (state.n ?? 0) + 1
  return state.n
}})

export const waits = action(async (note) => {{
  const done = await event("done", {{ expires: "1d" }})
  state.done = {{ text: note.text, how: done.how }}
}})

route.get("/done/:token", () => "done", {{ resumes: "done" }})
"#
    );
    let file = outside.join("kept.holon.ts");
    std::fs::write(&file, &text).unwrap();
    let file = file.display().to_string();
    let at = server.port.to_string();
    server.holon(&["deploy", &file, "--server", &at, "--approve"]);
    server.holon(&["flows", "save", &file, "--name", "kept-copy", "--server", &at]);
    let until = Instant::now() + Duration::from_secs(30);
    while status(&post(flow_port, "/bump", "{}")) != 200 {
        assert!(Instant::now() < until, "kept answers on {flow_port}");
        std::thread::sleep(Duration::from_millis(100));
    }
    let answered = post(flow_port, "/note", r#"{"text":"first"}"#);
    assert_eq!(status(&answered), 200, "{answered}");
    let resume = answered.rsplit("\r\n\r\n").next().unwrap().trim_matches('"').to_string();
    let waiting = post(flow_port, "/note", r#"{"text":"second"}"#);
    assert_eq!(status(&waiting), 200, "a run left waiting: {waiting}");
    let done = get(flow_port, &resume);
    assert_eq!(status(&done), 200, "the first run resumed at {resume}: {done}");
    // Past twice the 128 the hot file keeps (recorder.rs `HOT`): a cut seals the older ones.
    for _ in 0..300 {
        assert_eq!(status(&post(flow_port, "/bump", "{}")), 200);
    }
    // Console lines are written about once a second, on a thread of their own (logs.rs): looked
    // for until they are there, not slept for.
    let until = Instant::now() + Duration::from_secs(60);
    while !{
        let mut files = Vec::new();
        files_under(&home, &mut files);
        files.iter().any(|f| f.to_string_lossy().replace('\\', "/").ends_with("deployments.logs/kept.console.jsonl"))
    } {
        assert!(Instant::now() < until, "console lines written");
        std::thread::sleep(Duration::from_millis(100));
    }

    let mut files = Vec::new();
    files_under(&home, &mut files);
    let names = |pred: &dyn Fn(&str) -> bool| files.iter().any(|f| pred(&f.to_string_lossy().replace('\\', "/")));
    // The control: the kinds of file this means to look at are there.
    for (what, found) in [
        ("the run log", names(&|f| f.ends_with("deployments.logs/kept.jsonl"))),
        ("a sealed segment", names(&|f| f.contains("deployments.logs/kept.sealed/") && f.ends_with(".jsonl.gz"))),
        ("console lines", names(&|f| f.ends_with("deployments.logs/kept.console.jsonl"))),
        ("events", names(&|f| f.ends_with("deployments.logs/kept.events.jsonl"))),
        ("the deployments", names(&|f| f.ends_with("/deployments.json"))),
        ("saved flows", names(&|f| f.ends_with("/flows.json"))),
        ("the store's values", names(&|f| f.contains("/store/values/"))),
        ("the state head", names(&|f| f.ends_with("deployments.state/kept"))),
        ("the activity", names(&|f| f.ends_with("/deployments.activity.jsonl"))),
    ] {
        assert!(found, "{what} is among the files looked at: {files:#?}");
    }
    let open: Vec<&PathBuf> = files.iter().filter(|f| !holon_host::is_owners_alone(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()))).collect();
    // Said by folder, with an example each: a writer leaves many files alike (the store's values).
    let mut by_folder = std::collections::BTreeMap::<String, (usize, String)>::new();
    for f in &open {
        let folder = f.parent().unwrap().strip_prefix(&home).unwrap().display().to_string();
        let e = by_folder.entry(folder).or_insert((0, f.file_name().unwrap().to_string_lossy().into_owned()));
        e.0 += 1;
    }
    assert!(open.is_empty(), "readable by more than its owner, of {} files (folder: how many, one of them): {by_folder:#?}", files.len());

    // And every table in the data folder is sealed (at_rest.rs, step 2): only the keyring itself,
    // its keys wrapped, is kept as it is.
    let data = home.join("data");
    let tables: Vec<&PathBuf> = files.iter().filter(|f| f.parent() == Some(data.as_path()) && f.extension().is_some_and(|e| e == "json")).collect();
    assert!(tables.iter().any(|f| f.ends_with("deployments.json")) && tables.iter().any(|f| f.ends_with("keys.json")), "{tables:#?}");
    let plain: Vec<&&PathBuf> = tables.iter().filter(|f| !f.ends_with("keys.json") && !std::fs::read(f).unwrap().starts_with(b"HLA1")).collect();
    assert!(plain.is_empty(), "tables not sealed: {plain:#?}");

    // And every line log a line at a time (step 3): the run log, events, console lines, the
    // activity, the access history; a run log's segments whole. The witness's queue and receipts
    // (deployments.heads/) hold only hashes, and stay as they are (§2.5).
    let logs = data.join("deployments.logs");
    let line_logs: Vec<&PathBuf> = files
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "jsonl") && (f.parent() == Some(logs.as_path()) || f.parent() == Some(data.as_path())))
        .collect();
    for want in ["kept.jsonl", "kept.events.jsonl", "kept.console.jsonl", "deployments.activity.jsonl"] {
        assert!(line_logs.iter().any(|f| f.ends_with(want)), "{want} is among the line logs looked at: {line_logs:#?}");
    }
    for f in &line_logs {
        let text = std::fs::read_to_string(f).unwrap();
        assert!(!text.is_empty() && text.lines().all(|l| l.starts_with("a1.") || l.starts_with("r1.")), "{} holds a line not sealed", f.display());
    }
    let segments: Vec<&PathBuf> = files.iter().filter(|f| f.to_string_lossy().ends_with(".jsonl.gz")).collect();
    assert!(!segments.is_empty() && segments.iter().all(|f| std::fs::read(f).unwrap().starts_with(b"HLA1")), "segments sealed whole: {segments:#?}");

    // And the content store (step 4): its units, values and file texts each sealed whole, its
    // packs' blocks and index (a pack begins HOLONPK2); each named by 64 hex, a keyed hash.
    let store = data.join("store");
    let in_store: Vec<&PathBuf> = files.iter().filter(|f| f.starts_with(&store)).collect();
    assert!(in_store.iter().any(|f| f.parent() == Some(store.as_path())) && in_store.iter().any(|f| f.parent() == Some(store.join("values").as_path())), "units and values: {in_store:#?}");
    for f in &in_store {
        let bytes = std::fs::read(f).unwrap();
        let name = f.file_name().unwrap().to_string_lossy();
        if f.extension().is_some_and(|e| e == "pack") {
            assert!(bytes.starts_with(b"HOLONPK2"), "{} not a sealed pack", f.display());
        } else {
            assert!(name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit()), "{} not named by a keyed hash", f.display());
            assert!(bytes.starts_with(b"HLA1"), "{} not sealed", f.display());
        }
    }
}
