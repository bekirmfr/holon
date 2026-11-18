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
//!
//! An event whose line couldn't be written (a full disk) leaves the chain where it was, as a run
//! log's does: the next line written follows the last one that was and says from which event on
//! the lines before it were lost (`lost_since`), and the witness is sent no head that no line has.

use std::collections::BTreeMap;
use std::path::PathBuf;
use crate::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::paths::Place;
use crate::recorder::line_hash;
use crate::Deployment;

/// One event, as its line holds it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Event {
    pub(crate) seq: u64,
    /// The hash of the line before it (empty for the first).
    pub(crate) prev: String,
    /// When the lines of the events before it couldn't be written (a full disk, say): the seq of
    /// the first of them, each up to it lost. Its `prev` is the line before them, the last one
    /// written, so the chain goes on whole and an event lost reads as lost, not removed, as a run
    /// log's line does (`LogEntry::lost_since`). Absent when none was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lost_since: Option<u64>,
    pub(crate) at_ms: u64,
    /// The run log's next seq when it happened: it came after the runs before that one.
    pub(crate) run_seq: u64,
    /// `"deploy"`, `"restore"`, `"ask"` (an agent's restore, saved for a person to approve),
    /// `"grant"`, `"secret"`, `"binding"` or `"lifecycle"` (pausing, stopping and running it, and
    /// the server starting again after a crash with a state its last run didn't leave: `loaded`,
    /// or with a waiting run cut off as it went on, which is ended).
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
    /// The state the server loaded as it started again after a stop that wasn't one (a crash, a
    /// power loss), the last run here not having left it: its snapshot's hash. A run the stop cut
    /// off between its commit and its line may have (persist.rs, `note_cut_off_runs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) loaded: Option<String>,
    /// What the person who deployed it said, kept with the grant it made: why they approved it,
    /// what they checked (a deploy's `note`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
}

/// What an event says beyond its kind and words.
#[derive(Default)]
pub(crate) struct Detail {
    pub(crate) version: Option<u32>,
    pub(crate) session: Option<u32>,
    pub(crate) name: Option<String>,
    pub(crate) from: Option<String>,
    pub(crate) restores: Vec<String>,
    pub(crate) loaded: Option<String>,
    pub(crate) note: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Events {
    entries: Vec<Event>,
    /// Where they are kept beside memory (none: memory only, a `:memory:` deployments file).
    file: Option<PathBuf>,
    /// How its file's lines are sealed: each under its deployment's keys (at_rest.rs, §5.2).
    line_seal: Option<crate::at_rest::LineSeal>,
    /// The hash of the newest line: the next event's `prev`.
    head: String,
    /// The seqs of the events that don't follow the line before them: the file was changed outside
    /// holon there.
    breaks: Vec<u64>,
    /// Whether the file ends in a line a crash cut short, which the next line mustn't join.
    cut: bool,
    slug: Option<String>,
    /// Whether events were written since the file was last synced to disk (recorder.rs's
    /// `sync_logs`).
    dirty: bool,
    /// Why its file isn't read or written ([`Events::refused`]).
    refused: Option<String>,
    /// The first event whose line couldn't be written since the last that was: the next line
    /// written says so (`Event::lost_since`).
    lost_since: Option<u64>,
}

/// Where `slug`'s events are kept for the deployments file at `deployments_path`, if anywhere:
/// beside its run log.
fn file_for(deployments_path: &Place, slug: &str) -> Option<PathBuf> {
    crate::recorder::file_for(deployments_path, slug).map(|p| p.with_extension("events.jsonl"))
}

impl Events {
    /// The events of deployment `slug`, read back from beside the deployments file at
    /// `deployments_path`, their chain checked.
    pub(crate) fn for_deployment(deployments_path: &Place, slug: &str) -> Events {
        let mut events = Events::open(file_for(deployments_path, slug));
        events.slug = Some(slug.to_string());
        events
    }

    fn open(file: Option<PathBuf>) -> Events {
        // `<slug>.events.jsonl`: a slug has no dot.
        let slug = |f: &std::path::Path| f.file_name().map(|n| n.to_string_lossy().split('.').next().unwrap_or_default().to_string()).unwrap_or_default();
        let line_seal = file.as_deref().map(|f| crate::at_rest::LineSeal::of(&slug(f), crate::at_rest::role::EVENTS));
        let mut events = Events { file, line_seal, ..Events::default() };
        let (Some(path), Some(seal)) = (&events.file, &events.line_seal) else { return events };
        // Refused, as a run log is (recorder.rs, `Log::refused`): in memory only, so nothing of
        // its file is numbered over or written, and who opens it doesn't run the deployment.
        let refuse = |why: String| Events { refused: Some(why), ..Events::default() };
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return events,
            Err(e) => return refuse(format!("its file {} can't be read: {e}", path.display())),
        };
        let text = String::from_utf8_lossy(&bytes);
        events.cut = !text.is_empty() && !text.ends_with('\n');
        // A line a crash cut short (or changed outside holon) doesn't open, and is skipped; the
        // chain goes on from the line before it, and says so at the next. One whose key is
        // missing is whole: never skipped.
        let mut lines = Vec::new();
        for (n, line) in text.lines().enumerate() {
            match seal.read(line) {
                Ok(plain) => lines.push(plain),
                Err(e) if e.key_missing() => {
                    return refuse(format!("line {} of {} is {e}: {}", n + 1, path.display(), crate::at_rest::RESTORE_KEYS));
                }
                Err(_) => {}
            }
        }
        for line in lines {
            if let Ok(event) = serde_json::from_str::<Event>(&line) {
                if event.prev != events.head {
                    events.breaks.push(event.seq);
                }
                events.head = line_hash(&line);
                events.entries.push(event);
            }
        }
        events
    }

    /// Why its file isn't read or written, if it isn't: a line whose key `keys.json` doesn't have,
    /// or a file that couldn't be read.
    pub(crate) fn refused(&self) -> Option<&str> {
        self.refused.as_deref()
    }

    /// Append an event, numbered and chained to the one before; keep it in the file.
    pub(crate) fn push(&mut self, kind: &str, what: String, by: Option<&str>, run_seq: u64, detail: Detail) {
        let at_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let event = Event {
            seq: self.entries.last().map_or(0, |e| e.seq + 1),
            prev: self.head.clone(),
            lost_since: self.lost_since,
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
            loaded: detail.loaded,
            note: detail.note,
        };
        let line = match serde_json::to_string(&event) {
            Ok(line) => line,
            Err(e) => {
                eprintln!("holon: could not write an event: {e}");
                return;
            }
        };
        let seq = event.seq;
        self.entries.push(event);
        // The chain goes on from lines in the file, and the witness is sent only their heads: a
        // line that couldn't be written is said lost by the next that is (release audit D8), never
        // named as the `prev` of one that would read as the chain broken.
        match self.keep_line(&line) {
            Ok(()) => {
                self.head = line_hash(&line);
                self.lost_since = None;
                if let Some(slug) = &self.slug {
                    crate::heads::note(crate::heads::Head { deployment: slug.clone(), kind: "event".into(), seq, head: self.head.clone(), at_ms });
                }
            }
            Err(e) => {
                eprintln!("holon: could not keep an event in {:?}: {e}", self.file);
                self.lost_since.get_or_insert(seq);
            }
        }
    }

    fn keep_line(&mut self, line: &str) -> std::io::Result<()> {
        let Some(path) = self.file.clone() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let line = match &self.line_seal {
            Some(seal) => seal.seal(line)?,
            None => line.to_string(),
        };
        let after_cut = std::mem::take(&mut self.cut);
        let text = if after_cut { format!("\n{line}\n") } else { format!("{line}\n") };
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
        let bytes = text.as_bytes();
        let failing = crate::faults::failing("an event");
        let bytes = if failing == Some(true) { &bytes[..bytes.len() / 2] } else { bytes };
        let (put, wrote) = match failing {
            Some(false) => (0, Err(crate::faults::error("an event"))),
            Some(true) => (crate::recorder::write_counted(&mut file, bytes).0, Err(crate::faults::error("an event"))),
            None => crate::recorder::write_counted(&mut file, bytes),
        };
        // Part of the line may be in the file: the next starts on a line of its own, as after a
        // crash, so it isn't lost joined to the part.
        self.dirty |= put > 0;
        if let Err(e) = wrote {
            self.cut = after_cut || put > 0;
            return Err(e);
        }
        Ok(())
    }

    /// The file to sync, if events were written since it last was: the flag is cleared, and put
    /// back with `still_dirty` if the sync fails.
    pub(crate) fn take_dirty(&mut self) -> Option<PathBuf> {
        std::mem::take(&mut self.dirty).then(|| self.file.clone()).flatten()
    }

    /// Its file still needs syncing: the last sync of it failed.
    pub(crate) fn still_dirty(&mut self) {
        self.dirty = true;
    }

    /// Every event, oldest first.
    pub(crate) fn entries(&self) -> &[Event] {
        &self.entries
    }

    /// The lines exactly as made, oldest first, so a copy checks against the chain: each opened
    /// (one that doesn't is left out, and the chain says so at the next).
    pub(crate) fn lines(&self) -> Vec<String> {
        if let (Some(path), Some(seal)) = (&self.file, &self.line_seal) {
            return seal.read_lines(path);
        }
        self.entries.iter().filter_map(|e| serde_json::to_string(e).ok()).collect()
    }

    /// [`Events::lines`], an error if its file is there and can't be read, or a line's key is
    /// missing (`LineSeal::try_read_lines`): for what reads them to tell what they name.
    pub(crate) fn try_lines(&self) -> std::io::Result<Vec<String>> {
        if let (Some(path), Some(seal)) = (&self.file, &self.line_seal) {
            return seal.try_read_lines(path);
        }
        Ok(self.entries.iter().filter_map(|e| serde_json::to_string(e).ok()).collect())
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
///
/// Every viewer of the flow reads its timeline, and where a binding is goes only to whoever may
/// bind (web console L4, Q4): a connection's error that names one (a feed's address, a mail
/// server's) names the binding instead (folders.rs's `unplaced`, audit R51). Its bindings are read
/// with the table's lock let go, so no binding's lock is taken under it.
pub(crate) fn note(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, kind: &str, what: String, by: Option<&str>, detail: Detail) {
    let Some(folders) = deployments.lock().unwrap().get(slug).map(|d| std::sync::Arc::clone(&d.folders.all)) else { return };
    let what = crate::folders::unplaced(&what, &folders.whereabouts(slug));
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

        // Sealed on disk, each line (at_rest.rs).
        assert!(std::fs::read_to_string(&path).unwrap().lines().all(|l| l.starts_with("a1.")));
        // A line changed outside holon breaks the chain at the next one: changed and sealed again,
        // or changed as it is on disk, so it doesn't open.
        let lines = crate::at_rest::testing::open_lines(&path);
        crate::at_rest::testing::write_lines(&path, &lines.iter().map(|l| l.replace("paused", "resumed")).collect::<Vec<_>>());
        assert_eq!(Events::open(Some(path.clone())).breaks(), &[1]);
        crate::at_rest::testing::write_lines(&path, &lines);
        assert_eq!(Events::open(Some(path.clone())).breaks(), &[] as &[u64]);
        let mut raw: Vec<String> = std::fs::read_to_string(&path).unwrap().lines().map(str::to_string).collect();
        raw[0].insert(10, 'x');
        std::fs::write(&path, raw.join("\n") + "\n").unwrap();
        assert_eq!(Events::open(Some(path.clone())).breaks(), &[1]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
