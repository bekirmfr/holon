//! A deployment's events: what a person, an agent or the host did to it, each with who and when
//! (docs/L4s/history-L4.md §2). Deploys, restores, grant changes, secrets, bindings for it, and
//! pausing, stopping and running it. Beside its runs, they are its timeline.
//!
//! Recorded, never restored: a restore puts back code and state, and grants, secrets and bindings
//! are always as they are now.
//!
//! Kept beside the run log (`<slug>.events.jsonl`), hash chained the same way (each line's `prev`,
//! checked when it is opened), and kept for good: events are few, runs many. Each names the run
//! log's next seq when it happened (`run_seq`), so the two merge into one order. Each head goes to
//! the witness, if one is set (heads.rs, kind `"event"`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::recorder::line_hash;
use crate::Deployment;

/// One event, as its line holds it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Event {
    pub(crate) seq: u64,
    /// The hash of the line before it (empty for the first).
    pub(crate) prev: String,
    pub(crate) at_ms: u64,
    /// The run log's next seq when it happened: it came after the runs before that one.
    pub(crate) run_seq: u64,
    /// `"deploy"`, `"restore"`, `"ask"` (an agent's restore, saved for a person to approve),
    /// `"grant"`, `"secret"`, `"binding"` or `"lifecycle"`.
    pub(crate) kind: String,
    /// What happened, in a few words: `revoked StateWrite("a")`, `paused`.
    pub(crate) what: String,
    /// Who: an account, `local:<user>`, `agent:<name>`; none for the host's own (a policy that no
    /// longer covers it pausing it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) by: Option<String>,
    /// The version it deployed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<u32>,
    /// The session it started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session: Option<u32>,
    /// The secret, folder or database it was about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    /// The point whose state a restore put back (`"version 3"`, `"run 41"`): where going back to
    /// the restore goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) from: Option<String>,
    /// What an ask would restore of its point (`from`): `"code"`, `"state"`, or both.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) restores: Vec<String>,
}

/// What an event says beyond its kind and words.
#[derive(Default)]
pub(crate) struct Detail {
    pub(crate) version: Option<u32>,
    pub(crate) session: Option<u32>,
    pub(crate) name: Option<String>,
    pub(crate) from: Option<String>,
    pub(crate) restores: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Events {
    entries: Vec<Event>,
    /// Where they are kept beside memory (none: memory only, a `:memory:` deployments file).
    file: Option<PathBuf>,
    /// The hash of the newest line: the next event's `prev`.
    head: String,
    /// The seqs of the events that don't follow the line before them: the file was changed outside
    /// holon there.
    breaks: Vec<u64>,
    /// Whether the file ends in a line a crash cut short, which the next line mustn't join.
    cut: bool,
    slug: Option<String>,
}

/// Where `slug`'s events are kept for the deployments file at `deployments_path`, if anywhere:
/// beside its run log.
fn file_for(deployments_path: &str, slug: &str) -> Option<PathBuf> {
    crate::recorder::file_for(deployments_path, slug).map(|p| p.with_extension("events.jsonl"))
}

impl Events {
    /// The events of deployment `slug`, read back from beside the deployments file at
    /// `deployments_path`, their chain checked.
    pub(crate) fn for_deployment(deployments_path: &str, slug: &str) -> Events {
        let mut events = Events::open(file_for(deployments_path, slug));
        events.slug = Some(slug.to_string());
        events
    }

    fn open(file: Option<PathBuf>) -> Events {
        let mut events = Events { file, ..Events::default() };
        let Some(path) = &events.file else { return events };
        let Ok(text) = std::fs::read_to_string(path) else { return events };
        events.cut = !text.is_empty() && !text.ends_with('\n');
        for line in text.lines() {
            // A line a crash cut short is skipped; the chain goes on from the line before it.
            if let Ok(event) = serde_json::from_str::<Event>(line) {
                if event.prev != events.head {
                    events.breaks.push(event.seq);
                }
                events.head = line_hash(line);
                events.entries.push(event);
            }
        }
        events
    }

    /// Append an event, numbered and chained to the one before; keep it in the file.
    pub(crate) fn push(&mut self, kind: &str, what: String, by: Option<&str>, run_seq: u64, detail: Detail) {
        let at_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let event = Event {
            seq: self.entries.last().map_or(0, |e| e.seq + 1),
            prev: self.head.clone(),
            at_ms,
            run_seq,
            kind: kind.into(),
            what,
            by: by.map(String::from),
            version: detail.version,
            session: detail.session,
            name: detail.name,
            from: detail.from,
            restores: detail.restores,
        };
        let line = match serde_json::to_string(&event) {
            Ok(line) => line,
            Err(e) => {
                eprintln!("holon: could not write an event: {e}");
                return;
            }
        };
        self.head = line_hash(&line);
        if let Some(slug) = &self.slug {
            crate::heads::note(crate::heads::Head { deployment: slug.clone(), kind: "event".into(), seq: event.seq, head: self.head.clone(), at_ms });
        }
        self.entries.push(event);
        if let Err(e) = self.keep_line(&line) {
            eprintln!("holon: could not keep an event in {:?}: {e}", self.file);
        }
    }

    fn keep_line(&mut self, line: &str) -> std::io::Result<()> {
        use std::io::Write;
        let Some(path) = self.file.clone() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = if std::mem::take(&mut self.cut) { format!("\n{line}\n") } else { format!("{line}\n") };
        let new = !path.exists();
        let mut options = std::fs::OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        if new {
            crate::sealing::restrict_to_owner(&path)?;
        }
        file.write_all(text.as_bytes())
    }

    /// Every event, oldest first.
    pub(crate) fn entries(&self) -> &[Event] {
        &self.entries
    }

    /// The lines exactly as written, oldest first, so a copy checks against the chain.
    pub(crate) fn lines(&self) -> Vec<String> {
        if let Some(path) = &self.file {
            return std::fs::read_to_string(path).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default();
        }
        self.entries.iter().filter_map(|e| serde_json::to_string(e).ok()).collect()
    }

    /// The hash of the newest line.
    pub(crate) fn head(&self) -> &str {
        &self.head
    }

    pub(crate) fn breaks(&self) -> &[u64] {
        &self.breaks
    }

    /// Delete the file: its deployment was deleted.
    pub(crate) fn delete_file(&self) {
        if let Some(path) = &self.file {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Deployment {
    /// Note an event on this deployment's timeline, after the runs logged so far.
    pub(crate) fn note(&mut self, kind: &str, what: String, by: Option<&str>, detail: Detail) {
        let run_seq = self.log.next_seq();
        self.events.push(kind, what, by, run_seq, detail);
    }
}

/// Note an event on `slug`'s timeline, if it is deployed (a secret may be set before it is: the
/// secret's own audit holds that). Takes the deployments lock.
pub(crate) fn note(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, kind: &str, what: String, by: Option<&str>, detail: Detail) {
    if let Some(dep) = deployments.lock().unwrap().get_mut(slug) {
        dep.note(kind, what, by, detail);
    }
}

/// `GET /deployments/<slug>/events` — every event, oldest first, with the head of their chain and
/// the seqs where it breaks (the file was changed outside holon).
pub(crate) fn handle_get_events(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str) -> rouille::Response {
    match deployments.lock().unwrap().get(slug) {
        Some(dep) => rouille::Response::json(&serde_json::json!({
            "events": dep.events.entries(),
            "head": dep.events.head(),
            "breaks": dep.events.breaks(),
            "next_run": dep.log.next_seq(),
        })),
        None => crate::err(404, format!("no deployment {slug:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("holon-events-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("e.events.jsonl")
    }

    #[test]
    fn events_are_chained_kept_and_read_back() {
        let path = temp("chain");
        let mut events = Events::open(Some(path.clone()));
        events.push("lifecycle", "paused".into(), Some("maria"), 4, Detail::default());
        events.push("secret", "set slack_token (v2, env)".into(), Some("maria"), 4, Detail { name: Some("slack_token".into()), ..Detail::default() });
        let again = Events::open(Some(path.clone()));
        assert_eq!(again.entries(), events.entries());
        assert_eq!((again.head(), again.breaks()), (events.head(), &[][..]));
        assert_eq!(again.entries()[1].prev, line_hash(&again.lines()[0]));
        assert_eq!((again.entries()[1].seq, again.entries()[1].run_seq), (1, 4));

        // A line changed outside holon breaks the chain at the next one.
        let text = std::fs::read_to_string(&path).unwrap().replace("paused", "resumed");
        std::fs::write(&path, text).unwrap();
        assert_eq!(Events::open(Some(path.clone())).breaks(), &[1]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
