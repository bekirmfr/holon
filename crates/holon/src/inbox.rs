//! Mail starts runs (`on.mail(remit, (m) => …)`, a `mail_arrival` leaf; host-capabilities L4
//! §5.2.4). Every second a thread looks for the mailboxes running deployments listen to whose
//! binding's `--every` has come round, and starts a look at each on a thread of its own: it logs
//! in (imap.rs), selects the folder, lists its UIDs, and for each it hasn't seen, oldest first, at
//! most [`MAX_RUNS_PER_LOOK`] a look, fetches the message and starts one run of the handler, `m`
//! the message as core reads it (`holon_core::mail::received`, the binding's `--verdict-from`
//! saying whose verdict counts).
//!
//! - **One mailbox holds no other.** The watcher waits for no look: a server that doesn't answer
//!   (30 seconds a step, imap.rs), a message long to read or a long run hold their own mailbox,
//!   whose next look starts once its last has ended, and no one else's. How a look went is noted
//!   at the tick after it ends.
//!
//! - **New since the deploy.** What the folder holds when the deployment first looks is seen, not
//!   run on and not moved, as a folder's files are. What it has seen is kept beside its log
//!   (`mail-seen/<slug>/<mail>.json`), with the folder's `UIDVALIDITY` and where it is: a folder the
//!   server numbered anew, or a binding pointed at another one, is seen anew.
//! - **Moved after the run,** to the binding's `--done`, once its record is written: a run that
//!   failed too (its record says why). Its UID is noted before the move, so a message whose move
//!   failed, or a crash cut off, is moved at the next look, not run again.
//! - **A second copy** (a `message_id` among the last [`RECENT`] run on) is moved without a run,
//!   as is a message past [`MAX_MESSAGE_BYTES`]; the timeline says so.
//! - **Taken up before it is read.** A message is noted taken up once it is fetched, before it is
//!   read, and the note is dropped once its run is over: a look cut off in between (a restart, a
//!   crash, a failure reading it) leaves it, and the next look tries it again. Cut off [`TRIES`]
//!   times, it is moved without a run, and the timeline says so: a message that brings its look
//!   down is read twice, not at every look and after every restart. Moved back to the folder, it
//!   runs. A server that fails to hand it over takes no try.
//! - **Unreachable:** a look that fails is tried at the next; the timeline says so once, and again
//!   when the mailbox is read again.

use std::collections::VecDeque;
use std::time::Instant;
use std::path::{Path, PathBuf};

use crate::*;

/// How often the thread looks for mailboxes due.
const TICK: Duration = Duration::from_secs(1);

/// The most runs one look at one mailbox starts: a full folder is worked through over the next
/// looks.
const MAX_RUNS_PER_LOOK: usize = 20;

/// How many of the messages last run on a mailbox's `message_id`s are kept, to know a second copy.
const RECENT: usize = 1000;

/// The largest message a run reads: a request's largest body (dataplane.rs).
const MAX_MESSAGE_BYTES: u64 = 10 * 1024 * 1024;

/// How many looks may take up a message and be cut off before it is moved without a run.
pub(crate) const TRIES: u32 = 2;

/// What a deployment has seen of one mailbox.
#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
struct Seen {
    /// Where: the server's address and the folder, as bound.
    at: String,
    /// The folder's `UIDVALIDITY` when its UIDs were seen.
    validity: u32,
    /// The UIDs seen in the folder: there when it was first looked at, or run on.
    uids: BTreeSet<u32>,
    /// The UIDs whose run is over, not moved yet.
    to_move: BTreeSet<u32>,
    /// The UIDs taken up by a look and not done with, each with how many looks took it up.
    #[serde(default)]
    taken: BTreeMap<u32, u32>,
    /// The `message_id`s last run on, oldest first.
    recent: VecDeque<String>,
}

/// Where what `slug` has seen of its mailboxes is kept: none in memory.
fn seen_dir(deployments_path: &Place, slug: &str) -> Option<PathBuf> {
    deployments_path.beside("mail-seen").map(|dir| PathBuf::from(dir).join(slug))
}

/// What was seen of a mailbox: none yet if nothing is kept. One kept that doesn't open is an
/// error (at_rest.rs, `read_seen`): the look stops there, and nothing is run or marked seen.
fn read_seen(file: Option<&PathBuf>, memory: &Mutex<BTreeMap<PathBuf, Vec<u8>>>, key: &Path) -> Result<Seen, crate::failing::Failed> {
    match file {
        Some(f) => Ok(crate::at_rest::read_seen(f, &crate::at_rest::deployment_of(key), crate::at_rest::role::MAIL_SEEN)?.unwrap_or_default()),
        None => Ok(memory.lock().unwrap().get(key).and_then(|b| serde_json::from_slice(b).ok()).unwrap_or_default()),
    }
}

fn keep_seen(file: Option<&PathBuf>, memory: &Mutex<BTreeMap<PathBuf, Vec<u8>>>, key: &Path, seen: &Seen) {
    let bytes = serde_json::to_vec(seen).unwrap_or_default();
    let Some(file) = file else {
        memory.lock().unwrap().insert(key.to_path_buf(), bytes);
        return;
    };
    let written = std::fs::create_dir_all(file.parent().expect("a file in a folder")).and_then(|_| crate::at_rest::write_sealed(file, &crate::at_rest::deployment_of(key), crate::at_rest::role::MAIL_SEEN, &bytes));
    if let Err(e) = written {
        eprintln!("holon: what {} saw of its mailbox wasn't kept: {e}", file.display());
    }
}

/// Forget what `slug` saw of its mailboxes (it is deleted).
pub(crate) fn forget(deployments_path: &Place, slug: &str) {
    if let Some(dir) = seen_dir(deployments_path, slug) {
        crate::at_rest::seen_forgotten(deployments_path, &dir);
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// When each mailbox (by deployment and mail) is next due, and whether its last look failed.
#[derive(Default)]
pub(crate) struct Looks {
    due: BTreeMap<(String, String), (Instant, bool)>,
    /// What the looks' threads share with the watcher.
    shared: Arc<Shared>,
}

/// What a look's thread shares with the watcher.
#[derive(Default)]
struct Shared {
    /// What is seen, for a server whose deployments are kept nowhere (`:memory:`).
    memory: Mutex<BTreeMap<PathBuf, Vec<u8>>>,
    going: Mutex<Going>,
}

/// The looks going on.
#[derive(Default)]
struct Going {
    /// The mailboxes (by deployment and mail) a look is at.
    at: BTreeSet<(String, String)>,
    /// How the looks that ended went, for the next tick to note.
    ended: Vec<Looked>,
    /// A UID the next look to take it up is cut off at, there, as a crash would cut it (a test's).
    #[cfg(test)]
    cut_at: Option<u32>,
}

/// A look at one mailbox, on its thread: however it ends, a panic too, the mailbox is free for
/// its next look, and how it went waits for the watcher.
struct Looking {
    shared: Arc<Shared>,
    key: (String, String),
    every: u64,
    outcome: Option<Result<(), String>>,
}

impl Drop for Looking {
    fn drop(&mut self) {
        let outcome = self.outcome.take().unwrap_or_else(|| Err("a look at it was cut off".to_string()));
        let mut going = self.shared.going.lock().unwrap();
        going.at.remove(&self.key);
        going.ended.push((self.key.clone(), self.every, outcome));
    }
}

/// A deployment listening to a mailbox: its slug, the handler, unit, state, permit, secrets and
/// folders.
type Listening = (String, holon_core::MailHandler, Hash, Option<DeploymentState>, Arc<Permit>, Vault, Arc<folders::Scoped>);

/// How a look went: the deployment and mail, its binding's `--every` in seconds, and why it failed.
type Looked = ((String, String), u64, Result<(), String>);

/// One look at every mailbox running deployments listen to, due or not (but those a look is at
/// already), each ended before it returns, and noted.
#[cfg(test)]
pub(crate) fn run_mail(store: &SharedStore, deployments: &Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: &Place, looks: &mut Looks) {
    for (_, look) in look_at(store, deployments, deployments_path, looks, true) {
        let _ = look.join();
    }
    ended(deployments, looks);
}

/// One tick of the watcher: the looks that ended noted, and one started at each mailbox due (all
/// of them with `all`) no look is at. It returns at once: each look's mailbox, and its thread.
#[cfg(test)]
pub(crate) fn tick(store: &SharedStore, deployments: &Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: &Place, looks: &mut Looks, all: bool) -> Vec<((String, String), thread::JoinHandle<()>)> {
    look_at(store, deployments, deployments_path, looks, all)
}

/// The mailboxes a look is at.
#[cfg(test)]
pub(crate) fn going(looks: &Looks) -> BTreeSet<(String, String)> {
    looks.shared.going.lock().unwrap().at.clone()
}

/// Cut off the next look to take up `uid`, right after it does, as a crash would.
#[cfg(test)]
pub(crate) fn cut_off_at(looks: &Looks, uid: u32) {
    looks.shared.going.lock().unwrap().cut_at = Some(uid);
}

/// Start a look at each mailbox due (all of them with `all`) that no look is at, each on a
/// thread of its own, the looks that ended noted first: each look's mailbox, and its thread.
fn look_at(store: &SharedStore, deployments: &Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: &Place, looks: &mut Looks, all: bool) -> Vec<((String, String), thread::JoinHandle<()>)> {
    ended(deployments, looks);
    let listening: Vec<Listening> = deployments
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, d)| d.takes_runs(store))
        .flat_map(|(slug, d)| {
            let handlers = d.server.as_ref().map(|s| s.mails.clone()).unwrap_or_default();
            handlers.into_iter().map(|h| (slug.clone(), h, d.unit, d.state.clone(), Arc::clone(&d.permit), Arc::clone(&d.secrets), Arc::clone(&d.folders)))
        })
        .collect();
    looks.due.retain(|(slug, mail), _| listening.iter().any(|l| l.0 == *slug && l.1.mail == *mail));
    let now = Instant::now();
    let mut going = looks.shared.going.lock().unwrap();
    let due: Vec<Listening> = listening
        .into_iter()
        .filter(|l| {
            let key = (l.0.clone(), l.1.mail.clone());
            !going.at.contains(&key) && (all || looks.due.get(&key).is_none_or(|(at, _)| *at <= now))
        })
        .collect();
    going.at.extend(due.iter().map(|l| (l.0.clone(), l.1.mail.clone())));
    drop(going);
    due.into_iter()
        .map(|l| {
            let key = (l.0.clone(), l.1.mail.clone());
            let every = l.6.all.mails.resolve(&l.0, &l.1.mail).and_then(|b| b.mailbox).map_or(60, |m| m.every);
            let mut looking = Looking { shared: Arc::clone(&looks.shared), key: key.clone(), every, outcome: None };
            let (store, deployments, deployments_path) = (store.clone(), Arc::clone(deployments), deployments_path.clone());
            let thread = thread::spawn(move || {
                looking.outcome = Some(look_once(&store, &deployments, &deployments_path, &looking.shared, &l));
                drop(looking);
            });
            (key, thread)
        })
        .collect()
}

/// Note how the looks that ended went (a mailbox that can't be read once, and once it is read
/// again), and when each mailbox is next due: its binding's `--every` after its look ended.
fn ended(deployments: &Mutex<BTreeMap<String, Deployment>>, looks: &mut Looks) {
    let ended = std::mem::take(&mut looks.shared.going.lock().unwrap().ended);
    for ((slug, mail), every, outcome) in ended {
        let failing = looks.due.get(&(slug.clone(), mail.clone())).is_some_and(|(_, f)| *f);
        match &outcome {
            Err(why) if !failing => note(deployments, &slug, &mail, format!("couldn't read the mailbox of {mail}: {why}; trying again every {}", folders::span_text(every))),
            Ok(()) if failing => note(deployments, &slug, &mail, format!("reads the mailbox of {mail} again")),
            _ => {}
        }
        looks.due.insert((slug, mail), (Instant::now() + Duration::from_secs(every), outcome.is_err()));
    }
}

fn note(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, mail: &str, what: String) {
    let detail = crate::events::Detail { name: Some(mail.to_string()), ..Default::default() };
    crate::events::note(deployments, slug, "mail", what, None, detail);
}

/// One look at the mailbox `l` listens to: the runs its new messages start, each message moved once
/// its run is over. An error says why the mailbox couldn't be read (no login in it).
fn look_once(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, deployments_path: &Place, shared: &Shared, l: &Listening) -> Result<(), String> {
    let memory = &shared.memory;
    let (slug, handler, _, _, _, _, files) = l;
    let mail = &handler.mail;
    let mails = &files.all.mails;
    let binding = mails.resolve(slug, mail).ok_or("it isn't bound now")?;
    let (mut session, mailbox) = mails.open_mailbox(&binding)?;
    let why = |f: imap::Failure| f.why().to_string();
    let validity = session.select(&mailbox.folder).map_err(why)?;
    let uids = session.uids().map_err(why)?;
    let key = PathBuf::from(slug).join(format!("{mail}.json"));
    let file = seen_dir(deployments_path, slug).map(|d| d.join(format!("{mail}.json")));
    let read = read_seen(file.as_ref(), memory, &key).map_err(|e| crate::at_rest::seen_held(&format!("the mailbox of {mail}"), e));
    crate::at_rest::note_seen(deployments_path, file.as_deref(), read.as_ref().map(|_| ()));
    let mut seen = match read {
        Ok(seen) => seen,
        Err(held) => {
            session.logout();
            return Err(held.said());
        }
    };
    let keep = |seen: &Seen| keep_seen(file.as_ref(), memory, &key, seen);
    let at = format!("{} {}", mailbox.address, mailbox.folder);
    if seen.at != at || seen.validity != validity {
        // First looked at, or numbered anew: what is there now isn't new.
        let anew = if seen.at == at { "numbered anew by its server: " } else { "" };
        seen = Seen { at, validity, uids: uids.iter().copied().collect(), recent: std::mem::take(&mut seen.recent), ..Seen::default() };
        keep(&seen);
        let held = match uids.len() {
            1 => "1 message".to_string(),
            n => format!("{n} messages"),
        };
        note(deployments, slug, mail, format!("listens to the mailbox of {mail} (its folder {}), {anew}what it holds now ({held}) isn't run on", mailbox.folder));
        session.logout();
        return Ok(());
    }
    let present: BTreeSet<u32> = uids.iter().copied().collect();
    // What a move missed: its run is over.
    seen.to_move.retain(|u| present.contains(u));
    seen.taken.retain(|u, _| present.contains(u));
    for uid in seen.to_move.clone() {
        session.move_to(uid, &mailbox.done).map_err(why)?;
        seen.to_move.remove(&uid);
        keep(&seen);
    }
    let new: Vec<u32> = uids.iter().copied().filter(|u| !seen.uids.contains(u)).take(MAX_RUNS_PER_LOOK).collect();
    for uid in new {
        let tries = seen.taken.get(&uid).copied().unwrap_or(0);
        if tries >= TRIES {
            note(deployments, slug, mail, format!("moved a message to {} without a run: the {TRIES} looks that took it up were cut off before its run was over (a restart, or a failure reading it); moved back to {}, it runs", mailbox.done, mailbox.folder));
            seen.uids.insert(uid);
            seen.to_move.insert(uid);
            seen.taken.remove(&uid);
            keep(&seen);
            session.move_to(uid, &mailbox.done).map_err(why)?;
            seen.to_move.remove(&uid);
            keep(&seen);
            continue;
        }
        // Gone since it was listed: nothing to run on.
        let Some(size) = session.size(uid).map_err(why)? else { continue };
        if size > MAX_MESSAGE_BYTES {
            note(deployments, slug, mail, format!("moved a message of {size} bytes to {} without a run: a run reads 10 MiB at most", mailbox.done));
        } else {
            let Some(raw) = session.fetch(uid).map_err(why)? else { continue };
            // Taken up before it is read: a message that brings its look down (its reading or
            // its run) is tried again, not at every look and after every restart.
            seen.taken.insert(uid, tries + 1);
            keep(&seen);
            #[cfg(test)]
            if shared.going.lock().unwrap().cut_at.take_if(|at| *at == uid).is_some() {
                panic!("a look cut off as it took up {uid} (a test's)");
            }
            let message = holon_core::mail::received(&raw, &mailbox.verdict_from);
            let id = match &message {
                Value::Record(m) => match m.get("message_id") {
                    Some(Value::Text(t)) => t.clone(),
                    _ => String::new(),
                },
                _ => String::new(),
            };
            if seen.recent.contains(&id) {
                note(deployments, slug, mail, format!("moved a second copy of <{id}> to {} without a run", mailbox.done));
            } else {
                fire_one_mail(store, deployments, deployments_path, l, message);
                seen.recent.push_back(id);
                while seen.recent.len() > RECENT {
                    seen.recent.pop_front();
                }
            }
        }
        // Noted before it is moved: a move a crash cuts off is made at the next look.
        seen.uids.insert(uid);
        seen.to_move.insert(uid);
        seen.taken.remove(&uid);
        keep(&seen);
        // Without MOVE or UIDPLUS it is copied and marked \Deleted: moved as far as it goes.
        session.move_to(uid, &mailbox.done).map_err(why)?;
        seen.to_move.remove(&uid);
        keep(&seen);
    }
    // What left the folder is forgotten.
    let before = seen.uids.len();
    seen.uids.retain(|u| present.contains(u));
    if seen.uids.len() != before {
        keep(&seen);
    }
    session.logout();
    Ok(())
}

/// One run of `l`'s handler for `message`, logged under its label.
fn fire_one_mail(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, deployments_path: &Place, l: &Listening, message: Value) {
    let (slug, handler, unit_hash, state, permit, secrets, files) = l;
    let resolver = match closure(store, *unit_hash) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("new mail [{slug}]: store error while materializing closure: {e:?}");
            record_trigger(deployments, slug, true);
            return;
        }
    };
    // The handler runs alone: its unit is the run's.
    let unit_hash = handler.handler;
    let label = handler.label.clone();
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        eprintln!("new mail [{slug}]: deployed unit no longer resolves in the store");
        record_trigger(deployments, slug, true);
        return;
    };
    let epoch_seconds = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let extra_sourced = [
        (Capability::MailArrival(handler.mail.clone()), message),
        (Capability::Clock, Value::Number(epoch_seconds as f64)),
        (Capability::Random, Value::Bytes(rand::random::<[u8; 32]>().to_vec())),
    ];
    let started = started_now();
    let channels = deployments.lock().unwrap().get(slug.as_str()).map(|d| Arc::clone(&d.channels));
    let declared = handler.log;
    let unrecorded = permit.log_level(Some(&label), declared) == holon_core::LogLevel::Unrecorded;
    let mut waits = RunWaits { folders: Some(Arc::clone(files)), channels, slug: Some(slug.to_string()), unrecorded, ..Default::default() };
    let resolver = holon_module::WithEngine::new(&resolver, holon_module::shared());
    let run = fire_with_state(root_unit, &[], &resolver, store, state.as_ref(), &extra_sourced, permit, true, secrets, &mut waits, None);
    let mut entry = run_entry("mail", started, &extra_sourced, &run);
    entry.route = Some(label.clone());
    match durable::park_if_waiting(deployments, store, slug, &unit_hash, "mail", Some(label.clone()), declared, &[], &extra_sourced, &run, &waits) {
        Ok(names) => entry.parked = names,
        Err(e) => entry.error = Some(e),
    }
    let parked_any = !entry.parked.is_empty();
    if let Some(l) = log_run(deployments, slug, entry, Redactor::tokens(&waits.tokens).and(Redactor::handed(&calls::handed(&run.rounds))), declared) {
        l.said(slug);
    }
    println!("new mail [{slug}] {label}: {} effect(s) performed", run.performed.len());
    record_trigger(deployments, slug, false);
    if parked_any {
        // Nobody waits on an answer: the server's output says so when it isn't saved.
        let _ = persist_deployments(deployments_path, deployments);
    }
}

/// The watcher: a thread that starts a look at each mailbox due every [`TICK`], and waits for
/// none of them.
pub(crate) fn spawn_watcher(store: SharedStore, deployments: Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: Place) {
    thread::spawn(move || {
        let mut looks = Looks::default();
        loop {
            thread::sleep(TICK);
            look_at(&store, &deployments, &deployments_path, &mut looks, false);
        }
    });
}
