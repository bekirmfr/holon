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
//! by the host's choice; or `"removed"` when the deployment is removed (its `seq` the run it would
//! have numbered next, its `head` its last run's): the slug deployed again starts a new chain at 0,
//! and the heads after it are of that one. No run data leaves the server: heads only. `batch`
//! names the batch, so a witness can take a retried one again harmlessly. With
//! `HOLON_EXPORT_HEADS_TOKEN` set, the request carries it as `Authorization: Bearer <token>`.
//!
//! A witness answering 2xx has taken the batch; whatever JSON it answers is its receipt, kept as a
//! line of `<deployments>.heads/receipts.jsonl` beside the logs (a witness that signs what it took
//! and when gives a receipt anyone can check). Until then the heads wait in
//! `<deployments>.heads/pending.jsonl`, so a restart or an outage loses none; a failing witness is
//! tried again after a growing pause, up to five minutes, and said so once until it recovers.
//!
//! A run queues its head in memory only, under a lock held for that alone: every run of every
//! deployment takes it, under its own log's lock, so nothing is written or synced under it (release
//! audit R45, D15). The queue file is written with the logs' sync, every second (`sync`), what came
//! since appended and synced, or the file written afresh after a batch is taken or the queue cut:
//! a stop or a crash loses what it loses of the logs, and at most the heads of the last second,
//! which the next head of each chain stands for. A receipt is synced as it is kept. While the witness is down the queue doesn't
//! grow without end: past [`MAX_PENDING`] heads, each deployment's newest of each kind stands for
//! the ones before it (`compact`), of each of its lives apart: a removal's head ends one. A head
//! stands for every line before it in its chain, so what the witness then takes pins every run it
//! would have, in fewer steps.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use crate::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// One head, as sent.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Head {
    pub deployment: String,
    /// `"run"`, `"event"`, `"trim"` or `"removed"` (see the module docs).
    pub kind: String,
    pub seq: u64,
    pub head: String,
    pub at_ms: u64,
}

/// The kind of a removal's head: it ends a deployment's chain (see the module docs).
pub(crate) const REMOVED: &str = "removed";

/// The most heads sent in one batch.
const MAX_BATCH: usize = 1000;
/// The most heads that wait before the queue is cut to each deployment's newest of each kind.
pub const MAX_PENDING: usize = 10_000;
/// The longest pause before trying a failing witness again.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Past this, `receipts.jsonl` is rolled to `receipts.1.jsonl`, the one before let go of: the
/// newest receipts are kept, up to twice this, and the file no longer grows for good (release
/// audit C12). A receipt answers for a batch of heads, each standing for every run before it in
/// its chain; the witness keeps every one it gave.
const RECEIPTS_ROLL_BYTES: u64 = 16 << 20;

pub struct Exporter {
    url: String,
    token: Option<String>,
    /// Where the queue and the receipts are kept (none: in memory only).
    dir: Option<PathBuf>,
    /// The heads not yet taken by the witness, oldest first; `pending.jsonl` holds them as of the
    /// last sync.
    pending: Mutex<Queue>,
    /// The queue file, written by the sync (see the module docs).
    file: Mutex<QueueFile>,
    /// How many may wait before the queue is cut ([`MAX_PENDING`]; fewer in a test).
    most: usize,
    agent: ureq::Agent,
    /// What makes the lines a batch's heads name durable, run before the batch is sent: the server
    /// syncs its logs (recorder.rs's `sync_logs`), so a witness never holds a head the disk lacks.
    before_send: OnceLock<Hook>,
    /// When the witness last took a batch (ms since the epoch; 0: not since the server started).
    taken_ms: std::sync::atomic::AtomicU64,
    /// Why the witness didn't take the last batch, while it doesn't: an admin's warning
    /// (storage.rs), never `/health`'s, since one witness down would take every server it
    /// witnesses out at once.
    failing: Mutex<Option<String>>,
}

/// The witness the heads go to, as the console's Overview says it (summary.rs): where (its
/// scheme, host and port, never a path, query or login), when it last took a batch since the
/// server started, and how many heads wait for it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Witness {
    pub address: String,
    pub taken_ms: Option<u64>,
    pub waiting: usize,
}

/// The witness, if heads are being exported.
pub fn witness() -> Option<Witness> {
    #[cfg(test)]
    if let Some(set) = in_test::get() {
        return Some(set);
    }
    let exporter = EXPORTER.get()?;
    let taken = exporter.taken_ms.load(std::sync::atomic::Ordering::Relaxed);
    Some(Witness { address: address_of(&exporter.url), taken_ms: (taken > 0).then_some(taken), waiting: exporter.pending.lock().unwrap().heads.len() })
}

/// The witness, and why it doesn't take the runs' heads, while it doesn't.
pub fn failing() -> Option<(Witness, String)> {
    EXPORTER.get()?.failing()
}

/// A URL's scheme, host and port: what's before its path, without a login.
fn address_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    if scheme.is_empty() { host.to_string() } else { format!("{scheme}://{host}") }
}

/// A step run before each batch is sent; an error sends nothing.
pub type Hook = Box<dyn Fn() -> Result<(), String> + Send + Sync>;

/// The heads waiting for the witness, in memory: nothing is written under its lock, which every
/// run takes (see the module docs).
#[derive(Default)]
struct Queue {
    heads: VecDeque<Head>,
    /// How many of the first heads `pending.jsonl` holds, in order: the next sync appends the rest.
    written: usize,
    /// The file is to be written afresh with every head that waits: a batch was taken off the
    /// front, the queue was cut, or a write failed.
    rewrite: bool,
    /// How many of the first heads are being sent: left as they are by a cut, since the batch's
    /// heads are taken off the front once the witness takes it.
    sending: usize,
}

/// `pending.jsonl`, open for appending the heads queued since it was last written, and whether it
/// was written since it was last synced: under a lock of its own, which the sync holds while it
/// writes, so two syncs write in order and no run waits on either.
#[derive(Default)]
struct QueueFile {
    /// Closed when the file is written afresh, so the next append opens the new one.
    file: Option<std::fs::File>,
    unsynced: bool,
}

static EXPORTER: OnceLock<Arc<Exporter>> = OnceLock::new();

/// Where the heads' queue and receipts are kept for the deployments file at `deployments_path`.
pub fn dir_for(deployments_path: &crate::paths::Place) -> Option<PathBuf> {
    deployments_path.beside("heads").map(PathBuf::from)
}

/// Send heads to `url` from now on, every `every`: the exporter every run's head goes to.
pub fn start(url: &str, token: Option<String>, dir: Option<PathBuf>, every: Duration) -> Result<(), String> {
    let exporter = Arc::new(Exporter::new(url, token, dir)?);
    EXPORTER.set(Arc::clone(&exporter)).map_err(|_| "heads are already being exported".to_string())?;
    std::thread::spawn(move || exporter.run(every));
    Ok(())
}

/// Run `hook` before each batch is sent to the witness, if heads are being exported: the server
/// syncs its logs there (docs/L4s/request-path-L4.md, C2 d).
pub fn before_send(hook: Hook) {
    if let Some(exporter) = EXPORTER.get() {
        exporter.before_send(hook);
    }
}

/// Queue `head` for the witness, if heads are being exported.
pub fn note(head: Head) {
    if let Some(exporter) = EXPORTER.get() {
        exporter.note(head);
    }
}

/// Sync the heads queued since the last sync to disk, if heads are being exported: with the logs,
/// every `SYNC_EVERY` (recorder.rs), and before the server stops.
pub fn sync() -> Result<(), String> {
    EXPORTER.get().map_or(Ok(()), |exporter| exporter.sync())
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
        let exporter = Exporter {
            url: url.to_string(),
            token: token.filter(|t| !t.is_empty()),
            dir,
            pending: Mutex::new(Queue { written: pending.len(), heads: pending, ..Queue::default() }),
            file: Mutex::new(QueueFile::default()),
            most: MAX_PENDING,
            agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build(),
            before_send: OnceLock::new(),
            taken_ms: std::sync::atomic::AtomicU64::new(0),
            failing: Mutex::new(None),
        };
        // A queue a server before this one let grow past the bound.
        let cut = {
            let mut queue = exporter.pending.lock().unwrap();
            (queue.heads.len() > exporter.most).then(|| exporter.compact(&mut queue))
        };
        if let Some(cut) = cut {
            exporter.said_cut(cut);
            if let Err(e) = exporter.sync() {
                eprintln!("holon: {e}");
            }
        }
        Ok(exporter)
    }

    /// Run `hook` before each batch is sent from now on (once: a second is ignored).
    pub fn before_send(&self, hook: Hook) {
        let _ = self.before_send.set(hook);
    }

    /// Queue `head`, in memory: written to the queue file by the next sync. Past `most` waiting,
    /// the queue cut ([`Exporter::compact`]). Nothing else is done under the queue's lock, which
    /// every run takes under its log's (see the module docs).
    pub fn note(&self, head: Head) {
        let cut = {
            let mut queue = self.pending.lock().unwrap();
            queue.heads.push_back(head);
            (queue.heads.len() > self.most).then(|| self.compact(&mut queue))
        };
        if let Some(cut) = cut {
            self.said_cut(cut);
        }
    }

    /// Keep of the waiting heads each deployment's newest of each kind, in the order they came,
    /// the queue file to be written afresh with them by the next sync: a run's head stands for
    /// every run before it in its log, an event's for every event before it, a cut-back's for
    /// every cut-back before it. Each of a slug's lives apart: a head before a removal's is of the
    /// chain removed, and one after it stands for none of that chain's runs (release audit R41).
    /// The heads of a batch being sent are left as they are, to be taken off once it is taken.
    /// How many waited, and how many wait now.
    fn compact(&self, queue: &mut Queue) -> (usize, usize) {
        let was = queue.heads.len();
        let mut seen = BTreeSet::new();
        // Newest first: each removal's head begins (going back) an older life of its deployment.
        let mut lives: BTreeMap<String, u32> = BTreeMap::new();
        let mut newest_of = |h: &Head| {
            let life = lives.entry(h.deployment.clone()).or_default();
            *life += u32::from(h.kind == REMOVED);
            seen.insert((h.deployment.clone(), h.kind.clone(), *life))
        };
        let newest: Vec<Head> = queue.heads.iter().skip(queue.sending).rev().filter(|h| newest_of(h)).cloned().collect();
        queue.heads.truncate(queue.sending);
        queue.heads.extend(newest.into_iter().rev());
        queue.rewrite = true;
        (was, queue.heads.len())
    }

    /// Say that the queue was cut, with no lock held.
    fn said_cut(&self, (was, now): (usize, usize)) {
        eprintln!("holon: {was} heads waited for the witness at {}: each deployment's newest stands for the ones before it, {now} wait now", address_of(&self.url));
    }

    /// Write the heads queued since the last sync to the queue file, then sync it: appended, or
    /// the file written afresh with every head that waits after a batch was taken or the queue
    /// cut. What is to be written is copied under the queue's lock, and written with only the
    /// file's held, so no run waits while it is written or synced. A write that fails has the
    /// next sync write the file afresh; a sync that fails is tried again the next time.
    pub fn sync(&self) -> Result<(), String> {
        let Some(dir) = &self.dir else { return Ok(()) };
        let mut file = self.file.lock().unwrap();
        let (whole, heads) = {
            let mut queue = self.pending.lock().unwrap();
            let whole = std::mem::take(&mut queue.rewrite);
            let from = if whole { 0 } else { queue.written.min(queue.heads.len()) };
            let heads: Vec<Head> = queue.heads.range(from..).cloned().collect();
            queue.written = queue.heads.len();
            (whole, heads)
        };
        let path = dir.join("pending.jsonl");
        let wrote = (|| {
            let mut text = String::new();
            for head in &heads {
                text.push_str(&serde_json::to_string(head).map_err(std::io::Error::other)?);
                text.push('\n');
            }
            std::fs::create_dir_all(dir)?;
            if whole {
                // Closed first: a handle would go on appending to the old file (and Windows won't
                // replace an open file). Written whole and synced.
                file.file = None;
                crate::sealing::write_private(&path, text.as_bytes())?;
                file.unsynced = false;
            } else if !text.is_empty() {
                if file.file.is_none() {
                    file.file = Some(crate::sealing::append_private(&path)?);
                }
                use std::io::Write;
                let written = file.file.as_mut().expect("opened above").write_all(text.as_bytes());
                if written.is_err() {
                    // Opened again for the next write, in case the file went away under it.
                    file.file = None;
                }
                written?;
                file.unsynced = true;
            }
            Ok::<(), std::io::Error>(())
        })();
        if let Err(e) = wrote {
            self.pending.lock().unwrap().rewrite = true;
            return Err(format!("the witness's queue couldn't be written: {e}"));
        }
        if file.unsynced {
            crate::recorder::sync_file(&path).map_err(|e| format!("the witness's queue couldn't be synced: {e}"))?;
            file.unsynced = false;
        }
        Ok(())
    }

    /// How many heads wait for the witness.
    pub fn waiting(&self) -> usize {
        self.pending.lock().unwrap().heads.len()
    }

    /// Send what waits now, a batch at a time: the number sent, or why the witness didn't take a
    /// batch (what it didn't take still waits, and why is kept until one is taken: `failing`).
    /// Heads queued meanwhile wait for the next time: a witness that runs on this server makes a
    /// head of its own for each batch it takes.
    pub fn flush(&self) -> Result<usize, String> {
        let flushed = self.send_waiting();
        *self.failing.lock().unwrap() = flushed.as_ref().err().cloned();
        flushed
    }

    /// The witness, and why it didn't take the last batch, while it doesn't.
    pub fn failing(&self) -> Option<(Witness, String)> {
        let why = self.failing.lock().unwrap().clone()?;
        let taken = self.taken_ms.load(std::sync::atomic::Ordering::Relaxed);
        Some((Witness { address: address_of(&self.url), taken_ms: (taken > 0).then_some(taken), waiting: self.waiting() }, why))
    }

    fn send_waiting(&self) -> Result<usize, String> {
        let mut left = self.waiting();
        let mut sent = 0;
        loop {
            let batch: Vec<Head> = {
                let mut queue = self.pending.lock().unwrap();
                let batch: Vec<Head> = queue.heads.iter().take(MAX_BATCH.min(left)).cloned().collect();
                queue.sending = batch.len();
                batch
            };
            if batch.is_empty() {
                return Ok(sent);
            }
            let receipt = self.send(&batch);
            {
                let mut queue = self.pending.lock().unwrap();
                queue.sending = 0;
                if receipt.is_ok() {
                    queue.heads.drain(..batch.len());
                    queue.rewrite = true;
                }
            }
            let (id, receipt) = receipt?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            self.taken_ms.store(now, std::sync::atomic::Ordering::Relaxed);
            // What still waits written afresh, and the receipt kept, with no run waiting on either.
            if let Err(e) = self.sync().and_then(|()| self.keep(&id, &receipt).map_err(|e| e.to_string())) {
                eprintln!("holon: the witness took a batch of heads, but keeping what still waits, or its receipt, failed: {e}");
            }
            sent += batch.len();
            left -= batch.len();
        }
    }

    /// Send `batch` to the witness, the lines its heads name made durable first: the batch's name,
    /// and what the witness answered, or why it didn't take it.
    fn send(&self, batch: &[Head]) -> Result<(String, String), String> {
        // Once taken, a head stays taken.
        if let Some(hook) = self.before_send.get() {
            hook()?;
        }
        let heads = serde_json::to_string(batch).map_err(|e| e.to_string())?;
        let id = blake3::hash(heads.as_bytes()).to_hex().to_string();
        let body = format!("{{\"batch\":\"{id}\",\"heads\":{heads}}}");
        let mut request = self.agent.post(&self.url).set("Content-Type", "application/json");
        if let Some(token) = &self.token {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        match request.send_string(&body) {
            Ok(response) => Ok((id, response.into_string().unwrap_or_default())),
            Err(ureq::Error::Status(code, response)) => {
                let why = response.into_string().unwrap_or_default();
                Err(format!("the witness answered {code}: {}", why.trim()))
            }
            Err(e) => Err(format!("the witness can't be reached: {}", crate::outbound::said(&e))),
        }
    }

    /// After a batch was taken: its receipt kept, synced.
    fn keep(&self, batch: &str, receipt: &str) -> std::io::Result<()> {
        self.keep_within(batch, receipt, RECEIPTS_ROLL_BYTES)
    }

    /// [`Exporter::keep`], `receipts.jsonl` rolled first if the receipt would take it past `roll`.
    fn keep_within(&self, batch: &str, receipt: &str, roll: u64) -> std::io::Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        // A receipt that isn't JSON is kept as text, under the batch it answered.
        let receipt: serde_json::Value = serde_json::from_str(receipt).unwrap_or_else(|_| serde_json::Value::String(receipt.to_string()));
        let line = serde_json::json!({ "batch": batch, "receipt": receipt }).to_string() + "\n";
        use std::io::Write;
        let path = dir.join("receipts.jsonl");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() + line.len() as u64 > roll) {
            std::fs::rename(&path, dir.join("receipts.1.jsonl"))?;
        }
        let mut receipts = crate::sealing::append_private(&path)?;
        receipts.write_all(line.as_bytes())?;
        // An append-only handle; the sync opens the file to write, as Windows asks.
        crate::recorder::sync_file(&dir.join("receipts.jsonl"))
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
        assert_eq!(exporter.sync(), Ok(()), "written to the queue file, as the logs' sync does each second");

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

    /// Guards: a witness that doesn't take the heads is an admin's warning saying where, why and
    /// how many wait, until it takes a batch again.
    /// Why: a witness down for days showed as one stderr line and nothing in the warnings (audit
    /// R39, EN's P5). It is never `/health`'s, since one witness down would take every server it
    /// witnesses out at once.
    /// If it fails: the witness fails in silence, or a witness that recovered goes on being
    /// reported.
    #[test]
    fn a_witness_that_doesnt_take_the_heads_is_a_warning_until_it_does() {
        let status = Arc::new(Mutex::new(503));
        let (url, _) = witness(Arc::clone(&status));
        let exporter = Exporter::new(&url, None, None).unwrap();
        exporter.note(head(0));
        assert!(exporter.failing().is_none(), "nothing sent yet");
        assert!(exporter.flush().is_err());
        let (at, why) = exporter.failing().expect("it failed");
        let warning = crate::storage::witness_warning(&at, &why);
        assert_eq!(warning.key, "witness");
        assert!(warning.says.contains(&address_of(&url)) && warning.says.contains("503") && warning.says.contains(": 1 wait; it has taken none"), "{warning:?}");
        *status.lock().unwrap() = 200;
        assert_eq!(exporter.flush(), Ok(1));
        assert!(exporter.failing().is_none(), "it takes them again");
    }

    /// Guards: the receipts kept are the newest, rolled to one file before them past
    /// [`RECEIPTS_ROLL_BYTES`], so they take at most twice that however long the server runs.
    /// Why: `receipts.jsonl` was appended to for good: a busy witnessed server filled its disk
    /// with them (release audit C12).
    #[test]
    fn the_receipts_kept_are_the_newest_and_take_a_bounded_room() {
        let dir = std::env::temp_dir().join(format!("holon-receipts-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exporter = Exporter::new("http://127.0.0.1:9", None, Some(dir.clone())).unwrap();
        let roll = 2_000;
        for n in 0..500 {
            exporter.keep_within(&format!("batch-{n:04}"), "{\"ok\":true}", roll).unwrap();
        }
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
        let (now, before) = (read("receipts.jsonl"), read("receipts.1.jsonl"));
        assert!(now.len() as u64 <= roll && before.len() as u64 <= roll, "{} and {} bytes", now.len(), before.len());
        assert!(now.lines().last().unwrap().contains("batch-0499"), "the newest kept");
        let first_now: serde_json::Value = serde_json::from_str(now.lines().next().unwrap()).unwrap();
        let last_before: serde_json::Value = serde_json::from_str(before.lines().last().unwrap()).unwrap();
        let n = |v: &serde_json::Value| v["batch"].as_str().unwrap()[6..].parse::<u32>().unwrap();
        assert_eq!(n(&last_before) + 1, n(&first_now), "none lost between the two");
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

    /// The lines a batch's heads name are made durable before the batch goes: the hook runs before
    /// each POST, and a hook that fails sends nothing and leaves the heads waiting.
    #[test]
    fn a_batch_goes_only_after_its_hook_has_run() {
        let (url, got) = witness(Arc::new(Mutex::new(200)));
        let exporter = Exporter::new(&url, None, None).unwrap();
        let failing = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (fails, sees, taken) = (Arc::clone(&failing), Arc::clone(&seen), Arc::clone(&got));
        exporter.before_send(Box::new(move || {
            // What the witness had taken when the hook ran: nothing of this batch yet.
            sees.lock().unwrap().push(taken.lock().unwrap().len());
            if fails.load(std::sync::atomic::Ordering::SeqCst) { Err("the disk is full".into()) } else { Ok(()) }
        }));
        exporter.note(head(0));
        assert_eq!(exporter.flush(), Err("the disk is full".into()));
        assert_eq!((got.lock().unwrap().len(), exporter.waiting()), (0, 1), "nothing sent, the head still waits");
        failing.store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(exporter.flush(), Ok(1));
        assert_eq!(got.lock().unwrap().len(), 1);
        assert_eq!(*seen.lock().unwrap(), [0, 0], "the hook ran before each send");
    }

    /// The queue file is kept open, and opened again after the rewrite that follows a taken batch:
    /// heads queued afterwards land in the new file, so a restart reads back exactly the heads the
    /// witness hasn't taken.
    #[test]
    fn heads_queued_after_a_batch_was_taken_land_in_the_new_queue_file() {
        let dir = std::env::temp_dir().join(format!("holon-heads-reopen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (url, got) = witness(Arc::new(Mutex::new(200)));
        let exporter = Exporter::new(&url, None, Some(dir.clone())).unwrap();
        exporter.note(head(0));
        exporter.note(head(1));
        assert_eq!(exporter.flush(), Ok(2), "taken, and the queue file rewritten empty");
        exporter.note(head(2));
        exporter.note(head(3));
        assert_eq!(exporter.sync(), Ok(()));
        let again = Exporter::new(&url, None, Some(dir.clone())).unwrap();
        assert_eq!(again.waiting(), 2, "the heads queued after the rewrite are in the file");
        assert_eq!(again.flush(), Ok(2));
        let sent: Vec<Head> = got.lock().unwrap().iter().flat_map(|(_, b)| serde_json::from_value::<Vec<Head>>(b["heads"].clone()).unwrap()).collect();
        assert_eq!(sent, [head(0), head(1), head(2), head(3)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn head_of(deployment: &str, kind: &str, seq: u64) -> Head {
        Head { deployment: deployment.into(), kind: kind.into(), seq, head: format!("{deployment}-{kind}-{seq}"), at_ms: seq }
    }

    fn taken(got: &Taken) -> Vec<Head> {
        got.lock().unwrap().iter().flat_map(|(_, b)| serde_json::from_value::<Vec<Head>>(b["heads"].clone()).unwrap()).collect()
    }

    /// **Guards:** the witness's queue while the witness is down: `note`'s bound and `compact`.
    ///
    /// **Why:** the queue grew by a head a run, in memory and on disk, for as long as the witness
    /// was down. Cut, it must still give the witness every deployment's newest head of each kind,
    /// since that head is what pins every line before it, in the order they came.
    ///
    /// **If it fails:** a server whose witness is down fills its memory and disk; or, cut, the
    /// witness is never given a deployment's newest head, and the runs after the last one it
    /// holds can be changed with nothing to show it.
    #[test]
    fn a_witness_down_long_is_given_each_deployments_newest_head_of_each_kind_in_order() {
        let dir = std::env::temp_dir().join(format!("holon-heads-bound-{}-{}", std::process::id(), rand::random::<u32>()));
        let status = Arc::new(Mutex::new(503));
        let (url, got) = witness(Arc::clone(&status));
        let mut exporter = Exporter::new(&url, None, Some(dir.clone())).unwrap();
        exporter.most = 100;
        let mut noted = Vec::new();
        for seq in 0..1000u64 {
            let head = head_of(["a", "b", "c"][(seq % 3) as usize], ["run", "run", "event", "trim"][(seq / 3 % 4) as usize], seq);
            exporter.note(head.clone());
            noted.push(head);
            assert!(exporter.waiting() <= 100, "bounded: {} wait after {seq}", exporter.waiting());
        }
        assert!(exporter.flush().is_err());
        assert_eq!(exporter.sync(), Ok(()));
        let on_disk = std::fs::read_to_string(dir.join("pending.jsonl")).unwrap();
        assert_eq!(on_disk.lines().count(), exporter.waiting(), "the file holds what waits");
        assert_eq!(Exporter::new(&url, None, Some(dir.clone())).unwrap().waiting(), exporter.waiting(), "and a restart reads it back");

        *status.lock().unwrap() = 200;
        assert!(exporter.flush().is_ok());
        let sent = taken(&got);
        let at = |h: &Head| noted.iter().position(|n| n == h).unwrap_or_else(|| panic!("never noted: {h:?}"));
        assert!(sent.windows(2).all(|w| at(&w[0]) < at(&w[1])), "in the order they were noted");
        for (deployment, kind) in [("a", "run"), ("b", "event"), ("c", "trim"), ("a", "trim"), ("c", "run")] {
            let newest = noted.iter().rev().find(|h| h.deployment == deployment && h.kind == kind).unwrap();
            assert!(sent.contains(newest), "{deployment}'s newest {kind} head is given: {newest:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);

        // A slug removed and deployed again while the witness is down: each life's newest heads
        // are given, the removal's between them (release audit R41). The new chain numbers from 0
        // again, and its heads stand for none of the old one's runs.
        let (url, got) = witness(Arc::clone(&status));
        let mut exporter = Exporter::new(&url, None, None).unwrap();
        exporter.most = 10;
        *status.lock().unwrap() = 503;
        let mut noted = Vec::new();
        for (life, kind) in [(0, "run"), (0, "event"), (0, "run"), (0, REMOVED), (1, "run"), (1, "event"), (1, "run"), (1, REMOVED), (2, "run")] {
            for seq in 0..5u64 {
                if kind == REMOVED && seq > 0 {
                    break;
                }
                let head = Head { deployment: "d".into(), kind: kind.into(), seq, head: format!("{life}-{kind}-{seq}"), at_ms: seq };
                exporter.note(head.clone());
                noted.push((life, head));
            }
        }
        assert!(exporter.flush().is_err());
        *status.lock().unwrap() = 200;
        assert!(exporter.flush().is_ok());
        let sent = taken(&got);
        for life in 0..3 {
            for kind in ["run", "event", REMOVED] {
                if let Some((_, newest)) = noted.iter().rev().find(|(l, h)| *l == life && h.kind == kind) {
                    assert!(sent.contains(newest), "life {life}'s newest {kind} head is given: {newest:?} of {sent:?}");
                }
            }
        }
        assert!(exporter.waiting() == 0 && sent.len() <= 9, "cut all the same: {sent:?}");
    }

    /// **Guards:** a cut while a batch is out leaves that batch's heads as they are.
    ///
    /// **Why:** a batch is taken off the front of the queue once the witness takes it. Cut
    /// meanwhile (runs go on while it is sent), the front would be other heads, and taking the
    /// batch's off would drop heads never sent, the newest among them.
    ///
    /// **If it fails:** a deployment's newest head is dropped unsent, and the runs since the last
    /// head the witness holds can be changed with nothing to show it.
    #[test]
    fn a_cut_while_a_batch_is_sent_drops_none_of_the_heads_after_it() {
        let (url, got) = witness(Arc::new(Mutex::new(200)));
        let mut exporter = Exporter::new(&url, None, None).unwrap();
        exporter.most = 100;
        let exporter = Arc::new(exporter);
        let inside = Arc::downgrade(&exporter);
        exporter.before_send(Box::new(move || {
            if let Some(ex) = inside.upgrade() {
                for seq in 1000..1300 {
                    ex.note(head(seq));
                }
            }
            Ok(())
        }));
        for seq in 0..50 {
            exporter.note(head(seq));
        }
        assert_eq!(exporter.flush(), Ok(50), "the batch that was out");
        assert_eq!(taken(&got), (0..50).map(head).collect::<Vec<_>>());
        let rest: Vec<Head> = exporter.pending.lock().unwrap().heads.iter().cloned().collect();
        assert!(rest.iter().all(|h| h.seq >= 1000) && rest.len() <= 100, "none of the batch, and bounded: {rest:?}");
        assert_eq!(rest.last(), Some(&head(1299)), "the newest of what came meanwhile waits");
    }

    /// **Guards:** a run's head is queued in memory, and nothing is written or synced under the
    /// queue's lock (release audit R45, D15): a sync writing the queue file, however slowly, holds
    /// up no run. The sync writes what came since, appended; a write that fails has the next write
    /// the file afresh, so a restart reads back every head that waits.
    ///
    /// **Why:** every run of every deployment queues its head under its own log's lock. The queue
    /// was appended to under its lock, and written afresh and synced under it after each batch the
    /// witness took: every deployment's runs waited on the disk.
    ///
    /// **If it fails:** with a witness, a slow disk under the heads' folder stalls every flow; or
    /// a head queued is lost from the file, and a restart sends the witness less than it should.
    #[test]
    fn a_head_is_queued_in_memory_and_written_by_the_sync_which_no_run_waits_on() {
        let dir = std::env::temp_dir().join(format!("holon-heads-sync-{}-{}", std::process::id(), rand::random::<u32>()));
        let exporter = Arc::new(Exporter::new("http://127.0.0.1:9/heads", None, Some(dir.clone())).unwrap());
        let on_disk = || std::fs::read_to_string(dir.join("pending.jsonl")).map_or(0, |t| t.lines().count());
        {
            // A sync writing the file, its lock held as through a slow write or fsync.
            let _writing = exporter.file.lock().unwrap();
            let noting = Arc::clone(&exporter);
            let (done, noted) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                for seq in 0..3 {
                    noting.note(head(seq));
                }
                let _ = done.send(());
            });
            assert!(noted.recv_timeout(crate::tests::EVENTUALLY).is_ok(), "a run waited on the queue file being written");
        }
        assert_eq!(on_disk(), 0, "nothing written as heads are queued");
        assert_eq!(exporter.sync(), Ok(()));
        assert_eq!(on_disk(), 3, "the sync writes what came");
        exporter.note(head(3));
        assert_eq!(exporter.sync(), Ok(()));
        assert_eq!(on_disk(), 4, "and appends what came since");
        // A write that fails (a folder stands where the file was): the next one writes it afresh.
        exporter.note(head(4));
        exporter.file.lock().unwrap().file = None;
        std::fs::remove_file(dir.join("pending.jsonl")).unwrap();
        std::fs::create_dir(dir.join("pending.jsonl")).unwrap();
        assert!(exporter.sync().is_err(), "a file that can't be written");
        std::fs::remove_dir(dir.join("pending.jsonl")).unwrap();
        assert_eq!(exporter.sync(), Ok(()));
        let again = Exporter::new("http://127.0.0.1:9/heads", None, Some(dir.clone())).unwrap();
        assert_eq!(again.pending.lock().unwrap().heads, (0..5).map(head).collect::<VecDeque<_>>(), "every head that waits, after a restart");
        let _ = std::fs::remove_dir_all(&dir);
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

/// A witness this thread's summaries say is set, for a test: the exporter is the process's, and
/// one a test started would be sent every other test's heads.
#[cfg(test)]
pub(crate) mod in_test {
    use super::Witness;
    use std::cell::RefCell;

    thread_local!(static SET: RefCell<Option<Witness>> = const { RefCell::new(None) });

    pub(crate) fn set(witness: Option<Witness>) {
        SET.with(|s| *s.borrow_mut() = witness);
    }

    pub(super) fn get() -> Option<Witness> {
        SET.with(|s| s.borrow().clone())
    }
}
