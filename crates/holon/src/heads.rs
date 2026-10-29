//! Sending the run logs' heads to a witness outside this server (`--export-heads URL`).
//!
//! A run log is a hash chain (recorder.rs): its head, the hash of its newest line, stands for every
//! run before it. A copy of each head held somewhere else is what makes the log's past fixed: a run
//! changed, removed or cut off the end afterwards no longer reaches a head the witness has. So
//! after each run its head is queued, and every few seconds the queue goes to the witness in one
//! `POST`, as JSON:
//!
//! ```json
//! { "batch": "<hash of the heads, hex>",
//!   "heads": [{ "deployment": "hello-api", "kind": "run", "seq": 7, "head": "<hex>", "at_ms": 1792… }] }
//! ```
//!
//! `kind` is `"run"` for a run's head, `"event"` for the head of the deployment's events (events.rs:
//! their own chain, `seq` the event's), or `"trim"` when a log is cut back to what it keeps (its
//! `seq` the oldest run kept, its `head` the hash of the last run dropped): runs before it are gone
//! by the host's choice. No run data leaves the server: heads only. `batch` names the batch, so a
//! witness can take a retried one again harmlessly. With `HOLON_EXPORT_HEADS_TOKEN` set, the request
//! carries it as `Authorization: Bearer <token>`.
//!
//! A witness answering 2xx has taken the batch; whatever JSON it answers is its receipt, kept as a
//! line of `<deployments>.heads/receipts.jsonl` beside the logs (a witness that signs what it took
//! and when gives a receipt anyone can check). Until then the heads wait in
//! `<deployments>.heads/pending.jsonl`, so a restart or an outage loses none; a failing witness is
//! tried again after a growing pause, up to five minutes, and said so once until it recovers.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// One head, as sent.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Head {
    pub deployment: String,
    /// `"run"`, `"event"` or `"trim"` (see the module docs).
    pub kind: String,
    pub seq: u64,
    pub head: String,
    pub at_ms: u64,
}

/// The most heads sent in one batch.
const MAX_BATCH: usize = 1000;
/// The longest pause before trying a failing witness again.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

pub struct Exporter {
    url: String,
    token: Option<String>,
    /// Where the queue and the receipts are kept (none: in memory only).
    dir: Option<PathBuf>,
    /// The heads not yet taken by the witness, oldest first, kept in step with `pending.jsonl`.
    pending: Mutex<VecDeque<Head>>,
    agent: ureq::Agent,
}

static EXPORTER: OnceLock<Arc<Exporter>> = OnceLock::new();

/// Where the heads' queue and receipts are kept for the deployments file at `deployments_path`.
pub fn dir_for(deployments_path: &str) -> Option<PathBuf> {
    (deployments_path != ":memory:").then(|| Path::new(deployments_path).with_extension("heads"))
}

/// Send heads to `url` from now on, every `every`: the exporter every run's head goes to.
pub fn start(url: &str, token: Option<String>, dir: Option<PathBuf>, every: Duration) -> Result<(), String> {
    let exporter = Arc::new(Exporter::new(url, token, dir)?);
    EXPORTER.set(Arc::clone(&exporter)).map_err(|_| "heads are already being exported".to_string())?;
    std::thread::spawn(move || exporter.run(every));
    Ok(())
}

/// Queue `head` for the witness, if heads are being exported.
pub fn note(head: Head) {
    if let Some(exporter) = EXPORTER.get() {
        exporter.note(head);
    }
}

impl Exporter {
    /// An exporter to `url` (https, or http to this machine), with the heads `dir` still holds.
    pub fn new(url: &str, token: Option<String>, dir: Option<PathBuf>) -> Result<Exporter, String> {
        let local = ["http://127.0.0.1", "http://localhost", "http://[::1]"]
            .iter()
            .any(|p| url.strip_prefix(p).is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/'])));
        if !url.starts_with("https://") && !local {
            return Err(format!("--export-heads needs an https:// URL (or http:// to this machine), not {url:?}"));
        }
        let mut pending = VecDeque::new();
        if let Some(text) = dir.as_ref().and_then(|d| std::fs::read_to_string(d.join("pending.jsonl")).ok()) {
            // A line a crash cut short is dropped; the heads after it carry the chain anyway.
            pending.extend(text.lines().filter_map(|l| serde_json::from_str::<Head>(l).ok()));
        }
        Ok(Exporter {
            url: url.to_string(),
            token: token.filter(|t| !t.is_empty()),
            dir,
            pending: Mutex::new(pending),
            agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build(),
        })
    }

    /// Queue `head`, on disk too.
    pub fn note(&self, head: Head) {
        let mut pending = self.pending.lock().unwrap();
        if let Err(e) = self.append(&head) {
            eprintln!("holon: could not queue a run's head for the witness: {e}");
        }
        pending.push_back(head);
    }

    fn append(&self, head: &Head) -> std::io::Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        std::fs::create_dir_all(dir)?;
        let line = serde_json::to_string(head).map_err(std::io::Error::other)? + "\n";
        use std::io::Write;
        std::fs::OpenOptions::new().append(true).create(true).open(dir.join("pending.jsonl"))?.write_all(line.as_bytes())
    }

    /// How many heads wait for the witness.
    pub fn waiting(&self) -> usize {
        self.pending.lock().unwrap().len()
    }

    /// Send what waits now, a batch at a time: the number sent, or why the witness didn't take a
    /// batch (what it didn't take still waits). Heads queued meanwhile wait for the next time:
    /// a witness that runs on this server makes a head of its own for each batch it takes.
    pub fn flush(&self) -> Result<usize, String> {
        let mut left = self.waiting();
        let mut sent = 0;
        loop {
            let batch: Vec<Head> = self.pending.lock().unwrap().iter().take(MAX_BATCH.min(left)).cloned().collect();
            if batch.is_empty() {
                return Ok(sent);
            }
            let heads = serde_json::to_string(&batch).map_err(|e| e.to_string())?;
            let id = blake3::hash(heads.as_bytes()).to_hex().to_string();
            let body = format!("{{\"batch\":\"{id}\",\"heads\":{heads}}}");
            let mut request = self.agent.post(&self.url).set("Content-Type", "application/json");
            if let Some(token) = &self.token {
                request = request.set("Authorization", &format!("Bearer {token}"));
            }
            let receipt = match request.send_string(&body) {
                Ok(response) => response.into_string().unwrap_or_default(),
                Err(ureq::Error::Status(code, response)) => {
                    let why = response.into_string().unwrap_or_default();
                    return Err(format!("the witness answered {code}: {}", why.trim()));
                }
                Err(e) => return Err(e.to_string()),
            };
            let mut pending = self.pending.lock().unwrap();
            pending.drain(..batch.len());
            if let Err(e) = self.keep(&pending, &id, &receipt) {
                eprintln!("holon: the witness took a batch of heads, but keeping its receipt failed: {e}");
            }
            sent += batch.len();
            left -= batch.len();
        }
    }

    /// After a batch was taken: the queue file rewritten to what still waits, the receipt kept.
    fn keep(&self, pending: &VecDeque<Head>, batch: &str, receipt: &str) -> std::io::Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        std::fs::create_dir_all(dir)?;
        let mut rest = String::new();
        for head in pending {
            rest.push_str(&serde_json::to_string(head).map_err(std::io::Error::other)?);
            rest.push('\n');
        }
        crate::sealing::write_private(&dir.join("pending.jsonl"), rest.as_bytes())?;
        // A receipt that isn't JSON is kept as text, under the batch it answered.
        let receipt: serde_json::Value = serde_json::from_str(receipt).unwrap_or_else(|_| serde_json::Value::String(receipt.to_string()));
        let line = serde_json::json!({ "batch": batch, "receipt": receipt }).to_string() + "\n";
        use std::io::Write;
        std::fs::OpenOptions::new().append(true).create(true).open(dir.join("receipts.jsonl"))?.write_all(line.as_bytes())
    }

    /// Send every `every`, pausing longer while the witness fails.
    fn run(&self, every: Duration) {
        let mut pause = every;
        let mut failing = false;
        loop {
            std::thread::sleep(pause);
            match self.flush() {
                Ok(_) => {
                    if failing {
                        println!("holon: the witness at {} takes heads again", self.url);
                    }
                    failing = false;
                    pause = every;
                }
                Err(e) => {
                    if !failing {
                        eprintln!("holon: the witness at {} didn't take the runs' heads ({e}); they wait and are sent again", self.url);
                    }
                    failing = true;
                    pause = (pause * 2).min(MAX_BACKOFF);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(seq: u64) -> Head {
        Head { deployment: "d".into(), kind: "run".into(), seq, head: format!("h{seq}"), at_ms: seq }
    }

    /// What a test witness took: each batch with the Authorization header it came with.
    type Taken = Arc<Mutex<Vec<(Option<String>, serde_json::Value)>>>;

    /// A witness on a free port, answering `status` and keeping what it was sent.
    fn witness(status: Arc<Mutex<u16>>) -> (String, Taken) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&got);
        let server = rouille::Server::new("127.0.0.1:0", move |request| {
            let mut body = String::new();
            use std::io::Read;
            request.data().unwrap().read_to_string(&mut body).unwrap();
            let batch: serde_json::Value = serde_json::from_str(&body).unwrap();
            let code = *status.lock().unwrap();
            if code == 200 {
                kept.lock().unwrap().push((request.header("Authorization").map(str::to_string), batch.clone()));
            }
            rouille::Response::json(&serde_json::json!({ "took": batch["batch"] })).with_status_code(code)
        })
        .unwrap();
        let url = format!("http://{}/heads", server.server_addr());
        std::thread::spawn(move || server.run());
        (url, got)
    }

    #[test]
    fn heads_wait_on_disk_until_the_witness_takes_them_and_its_receipt_is_kept() {
        let dir = std::env::temp_dir().join(format!("holon-heads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let status = Arc::new(Mutex::new(503));
        let (url, got) = witness(Arc::clone(&status));

        let exporter = Exporter::new(&url, Some("t0ken".into()), Some(dir.clone())).unwrap();
        exporter.note(head(0));
        exporter.note(head(1));
        assert!(exporter.flush().unwrap_err().contains("503"));
        assert_eq!(exporter.waiting(), 2, "a witness that fails takes nothing");

        // A restart: the queue comes back from disk.
        let again = Exporter::new(&url, Some("t0ken".into()), Some(dir.clone())).unwrap();
        assert_eq!(again.waiting(), 2);
        *status.lock().unwrap() = 200;
        assert_eq!(again.flush(), Ok(2));
        assert_eq!(again.waiting(), 0);
        assert_eq!(Exporter::new(&url, None, Some(dir.clone())).unwrap().waiting(), 0, "nothing waits on disk either");

        let got = got.lock().unwrap();
        assert_eq!(got.len(), 1, "one batch");
        let (auth, batch) = &got[0];
        assert_eq!(auth.as_deref(), Some("Bearer t0ken"));
        let heads: Vec<Head> = serde_json::from_value(batch["heads"].clone()).unwrap();
        assert_eq!(heads, [head(0), head(1)]);
        let id = blake3::hash(serde_json::to_string(&heads).unwrap().as_bytes()).to_hex().to_string();
        assert_eq!(batch["batch"], id, "the batch is named by its heads");

        let receipts = std::fs::read_to_string(dir.join("receipts.jsonl")).unwrap();
        let receipt: serde_json::Value = serde_json::from_str(receipts.lines().next().unwrap()).unwrap();
        assert_eq!(receipt["batch"], id);
        assert_eq!(receipt["receipt"]["took"], id, "the witness's answer, as it gave it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_head_queued_while_sending_waits_for_the_next_time() {
        // A witness on the server it witnesses: each batch it takes is a run, with a head of its own.
        let exporter: Arc<OnceLock<Arc<Exporter>>> = Arc::new(OnceLock::new());
        let inside = Arc::clone(&exporter);
        let server = rouille::Server::new("127.0.0.1:0", move |_| {
            inside.get().unwrap().note(head(99));
            rouille::Response::json(&serde_json::json!({}))
        })
        .unwrap();
        let url = format!("http://{}/heads", server.server_addr());
        std::thread::spawn(move || server.run());
        let ex = exporter.get_or_init(|| Arc::new(Exporter::new(&url, None, None).unwrap()));
        ex.note(head(0));
        assert_eq!(ex.flush(), Ok(1), "only what waited when it began");
        assert_eq!(ex.waiting(), 1, "the witness's own head, for next time");
        assert_eq!(ex.flush(), Ok(1));
    }

    #[test]
    fn a_witness_is_reached_over_https_or_on_this_machine() {
        for ok in ["https://witness.example/heads", "http://127.0.0.1:9000/heads", "http://localhost/h", "http://[::1]:1/"] {
            assert!(Exporter::new(ok, None, None).is_ok(), "{ok}");
        }
        for bad in ["http://witness.example/heads", "http://localhost.evil.example/", "ftp://x", "witness"] {
            assert!(Exporter::new(bad, None, None).is_err(), "{bad}");
        }
    }
}
