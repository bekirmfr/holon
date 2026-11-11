//! Mail (host-capabilities L4 §5.2.2): what a flow sends with `ops.send({ to, subject, text })`,
//! through a mail resource an admin binds.
//!
//! - **Bound by an admin,** as a database is (`--mails`, owner-only; `/admin/mails`; `holon mail`):
//!   a name a flow imports (`import ops from "mail:ops"`) bound to an SMTP server, its URL with its
//!   login read from a source (`env:…`, `vault:…`) each time and never kept (smtp.rs); the sender,
//!   the admin's, never the flow's; and every address it may ever send to (`--to`, the ceiling),
//!   for the whole server or one deployment.
//! - **The grant** names each recipient the text writes (`MailSend("ops/approvals@acme.com")`), or
//!   `MailSend("ops/*")` for a computed one. A deploy is refused when a named one is outside the
//!   binding's `--to`, or the mail isn't bound ([`Mails::refuses`]); a run's mail with a recipient
//!   the grant doesn't cover isn't performed ([`ungranted`]).
//! - **Sent after the run,** as a channel's pack is: written to the spool (beside the bindings
//!   file, one file a mail, owner-only) before its run's record is, held until that record is on
//!   disk, then queued: a queue for each binding, sending in order on its own thread, so a slow
//!   server holds no run. A temporary failure (no connection, a 4xx) is tried again, after a
//!   minute, then 5, then 30, then each hour for a day, with the same `Message-ID`; a permanent one
//!   (a 5xx), a recipient the binding no longer allows, or a day of temporary ones, gives up. Each
//!   outcome is noted on the deployment's timeline. A restart sends what is in the spool whose run
//!   its log holds: a run whose record a crash lost sent nothing.
//! - **An unrecorded run's mail** is spooled and sent all the same, though its log doesn't hold it.
//! - **The log** keeps each message whole but its attachments, kept by size and hash ([`loggable`]).
//! - **Receiving** (§5.2.4): the same name, bound with `--imap` (beside `--smtp` or instead), names
//!   a mailbox: its URL with its login read from a source as the SMTP one is (imap.rs), the folder
//!   read, the one each message is moved to after its run, and whose `Authentication-Results`
//!   verdict on a sender counts. `on.mail(remit, …)` runs on each message that comes there
//!   (inbox.rs); a deploy is refused while the mail isn't bound with `--imap`, or another
//!   deployment listens to that mailbox.
//! - **OAuth** (§5.2.6): with `--oauth`, a server whose URL names a user and no password logs in as
//!   that user with a token the host asks for (oauth.rs), kept in memory: Microsoft 365, Google
//!   Workspace. A login a token was refused at drops it, so the next asks anew.

use std::collections::VecDeque;
use std::sync::OnceLock;
use crate::sync::{Condvar, RwLock};
use std::time::{Duration, Instant};

use rouille::{Request, Response};

use crate::http::{err, read_body};
use crate::permissions::Manifest;
use crate::secret_sources::{Reference, Sources};
use crate::smtp::{self, Failure};
use crate::{imap, oauth};
use zeroize::Zeroizing;

use crate::*;

/// What the error of a run that sent more mails than a run may starts with.
pub(crate) const SEND_FAILED: &str = "mail not sent: ";

/// One bound mail resource: what it sends through (empty when it only receives), and the mailbox
/// it receives in (none when it only sends).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Binding {
    /// Where the host reads the server's URL with its login from (`env:…`, `vault:…`), never the
    /// URL itself.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) smtp: String,
    /// Where that URL points, without its login (`smtps://smtp.example.com:465`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) address: String,
    /// The sender: `Treasury <treasury@acme.com>`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) from: String,
    /// Every address it may send to: addresses, `*@domain`, or `*`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) to: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mailbox: Option<Mailbox>,
    /// Where its token comes from, for a server whose URL names a user and no password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) oauth: Option<OAuth>,
}

/// How a mail's servers are logged in to with a token (`--oauth`, oauth.rs).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct OAuth {
    /// Where the host reads the token request (its URL with the client's id and secret) from,
    /// never the request itself.
    pub(crate) source: String,
    /// Where the token is asked for, without the client's login or the query
    /// (`https://login.microsoftonline.com/<tenant>/oauth2/v2.0/token`).
    pub(crate) address: String,
}

/// The mailbox a mail receives in (`--imap`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Mailbox {
    /// Where the host reads the IMAP server's URL with its login from, never the URL itself.
    pub(crate) imap: String,
    /// Where that URL points, without its login (`imaps://imap.example.com:993`).
    pub(crate) address: String,
    /// The folder read.
    pub(crate) folder: String,
    /// The folder each message is moved to once its run is over.
    pub(crate) done: String,
    /// Whose verdict on a sender counts: the `authserv-id` the mailbox's server writes in its
    /// `Authentication-Results` header. Empty: nobody's, so nothing is authenticated.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) verdict_from: String,
    /// How often the folder is looked at, in seconds.
    pub(crate) every: u64,
}

impl Mailbox {
    /// Which mailbox it is: two bindings reading the same folder of the same server's login are
    /// one, which one deployment listens to.
    pub(crate) fn identity(&self) -> (String, String) {
        (self.imap.clone(), self.folder.clone())
    }
}

/// The least time between two looks at a mailbox.
pub(crate) const EVERY_LEAST: u64 = 10;

/// What binding a mail asks for, as `/admin/mails` takes it: `smtp` with `from` and `to` to send,
/// `imap` (with `folder`, `done`, `verdict_from` and `every` when not the defaults) to receive.
#[derive(Deserialize, Default, Debug)]
pub(crate) struct Asked {
    #[serde(default)]
    pub(crate) smtp: Option<String>,
    #[serde(default)]
    pub(crate) from: Option<String>,
    /// Addresses, `*@domain` or `*`: a list, or one text with commas.
    #[serde(default)]
    pub(crate) to: Option<To>,
    #[serde(default)]
    pub(crate) imap: Option<String>,
    #[serde(default)]
    pub(crate) folder: Option<String>,
    #[serde(default)]
    pub(crate) done: Option<String>,
    #[serde(default)]
    pub(crate) verdict_from: Option<String>,
    /// `30s`, `1m`, `2h`.
    #[serde(default)]
    pub(crate) every: Option<String>,
    /// Where the token request is read from, for a server whose URL names a user and no password.
    #[serde(default)]
    pub(crate) oauth: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub(crate) enum To {
    One(String),
    Many(Vec<String>),
}

impl To {
    fn list(&self) -> Vec<String> {
        match self {
            To::One(s) => s.split(',').map(str::to_string).collect(),
            To::Many(v) => v.clone(),
        }
    }
}

/// Text with what's around it trimmed, none when that leaves nothing.
fn given(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

impl Binding {
    /// Whether `address` is one the binding may send to.
    pub(crate) fn allows(&self, address: &str) -> bool {
        self.to.iter().any(|p| holon_core::mail::within(address, p))
    }

    fn sender_address(&self) -> String {
        holon_core::mail::sender(&self.from).map(|(_, a)| a).unwrap_or_default()
    }
}

/// Every binding, as [`folders::Bound`]: the server's by name, each deployment's under
/// `@deployments`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Bound {
    #[serde(rename = "@deployments", default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) deployments: BTreeMap<String, BTreeMap<String, Binding>>,
    #[serde(flatten)]
    pub(crate) server: BTreeMap<String, Binding>,
}

/// The mail resources bound on this server, and the mail on its way through them.
#[derive(Clone)]
pub(crate) struct Mails(Arc<Inner>);

struct Inner {
    file: Option<PathBuf>,
    bound: RwLock<Bound>,
    /// What a binding's URL is read from: the server's environment and dotenv file, Vault.
    sources: Sources,
    /// Where mail on its way is kept, a file each, until it is delivered or given up; none: in
    /// memory only.
    spool: Option<PathBuf>,
    /// The mail of logged runs whose records may not be on disk yet, by deployment, in its log's
    /// order (the entry's seq): it is queued once they are.
    held: Mutex<BTreeMap<String, VecDeque<(u64, Queued)>>>,
    /// The mail to send, by binding (`ops`, or `app/ops` for a deployment's own).
    queues: Mutex<BTreeMap<String, Arc<Queue>>>,
    /// The deployments, whose timelines each outcome is noted on: set once the server has them,
    /// when the queues start sending.
    deployments: OnceLock<Arc<Mutex<BTreeMap<String, Deployment>>>>,
    /// How long it waits to try a mail again after its first, second, third and later temporary
    /// failures, and how long after its run it gives up.
    retry: ([Duration; 4], Duration),
    /// The tokens the bindings' `--oauth` gave, in memory only.
    tokens: oauth::Tokens,
}

impl std::fmt::Debug for Mails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mails").field("bound", &self.0.bound.read().unwrap()).finish()
    }
}

/// A binding's queue, and the thread's signal that it has mail.
#[derive(Default)]
struct Queue {
    items: Mutex<VecDeque<Queued>>,
    ready: Condvar,
}

/// A mail on its way: where it came from and goes, as its spool file's first line keeps it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Spooled {
    slug: String,
    mail: String,
    run: String,
    /// Its place among its run's mails.
    n: usize,
    /// When its run began, in milliseconds since the epoch: a day after, it is given up.
    at_ms: u64,
    /// Its recipients, `to` and `cc`.
    to: Vec<String>,
    subject: String,
    /// Its run isn't logged (an unrecorded handler's): a restart sends it all the same.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    unlogged: bool,
}

pub(crate) struct Queued {
    /// The queue it goes to: its binding's (`ops`, or `app/ops` for a deployment's own).
    key: String,
    spooled: Spooled,
    /// The message as it is sent (MIME).
    message: Vec<u8>,
    /// Its spool file, removed once it is delivered or given up.
    file: Option<PathBuf>,
    /// The temporary failures it has had.
    tries: u32,
    not_before: Instant,
}

impl Mails {
    /// None, kept nowhere: the tests, and a server told `--mails :memory:`.
    pub(crate) fn memory() -> Mails {
        Mails::with(None, Bound::default(), None)
    }

    fn with(file: Option<PathBuf>, bound: Bound, spool: Option<PathBuf>) -> Mails {
        let retry = ([60, 300, 1800, 3600].map(Duration::from_secs), Duration::from_secs(24 * 3600));
        Mails(Arc::new(Inner {
            file,
            bound: RwLock::new(bound),
            sources: Sources::none(),
            spool,
            held: Mutex::new(BTreeMap::new()),
            queues: Mutex::new(BTreeMap::new()),
            deployments: OnceLock::new(),
            retry,
            tokens: oauth::Tokens::default(),
        }))
    }

    /// The mail resources bound in `file` (none yet if it doesn't exist), the spool beside it.
    pub(crate) fn open(file: &str) -> Result<Mails, String> {
        if file == ":memory:" {
            return Ok(Mails::memory());
        }
        let path = PathBuf::from(file);
        let bound = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{file} isn't a mails file: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Bound::default(),
            Err(e) => return Err(format!("can't read {file}: {e}")),
        };
        let spool = path.with_file_name("mail-spool");
        Ok(Mails::with(Some(path), bound, Some(spool)))
    }

    /// These, reading a binding's URL from `sources`.
    pub(crate) fn with_sources(self, sources: Sources) -> Mails {
        let inner = Arc::into_inner(self.0).expect("set up before it is shared");
        Mails(Arc::new(Inner { sources, ..inner }))
    }

    /// These, trying again after `delays` and giving up after `give_up`: the tests'.
    #[cfg(test)]
    pub(crate) fn with_retry(self, delays: [Duration; 4], give_up: Duration) -> Mails {
        let inner = Arc::into_inner(self.0).expect("set up before it is shared");
        Mails(Arc::new(Inner { retry: (delays, give_up), ..inner }))
    }

    /// Every binding: the server's, and each deployment's.
    pub(crate) fn all(&self) -> Bound {
        self.0.bound.read().unwrap().clone()
    }

    /// The binding deployment `deployment` sees as `name`, its own or the server's, and its queue's
    /// key.
    fn resolve_keyed(&self, deployment: &str, name: &str) -> Option<(String, Binding)> {
        let bound = self.0.bound.read().unwrap();
        match bound.deployments.get(deployment).and_then(|d| d.get(name)) {
            Some(b) => Some((format!("{deployment}/{name}"), b.clone())),
            None => bound.server.get(name).map(|b| (name.to_string(), b.clone())),
        }
    }

    /// The binding deployment `deployment` sees as `name`: its own, else the server's.
    pub(crate) fn resolve(&self, deployment: &str, name: &str) -> Option<Binding> {
        self.resolve_keyed(deployment, name).map(|(_, b)| b)
    }

    /// Bind `name`, for the whole server or one deployment, replacing any binding of it there
    /// before, as `asked` says: to send (`smtp`, where its server's URL is read from, logged in to
    /// now), to receive (`imap`: logged in to now, its folder selected and `done` made), or both.
    pub(crate) fn bind(&self, name: &str, asked: &Asked, for_deployment: Option<&str>) -> Result<Binding, String> {
        if !folders::is_folder_name(name) || name.contains('-') {
            return Err(format!("{name:?} isn't a mail's name: letters, digits and _, up to 64, as a flow imports it"));
        }
        if let Some(slug) = for_deployment.filter(|s| !folders::is_folder_name(s)) {
            return Err(format!("{slug:?} isn't a deployment's name"));
        }
        let (smtp, imap) = (given(&asked.smtp), given(&asked.imap));
        if smtp.is_none() && imap.is_none() {
            return Err("a mail sends (--smtp) or receives (--imap), or both: it says neither".into());
        }
        if imap.is_none() && [&asked.folder, &asked.done, &asked.verdict_from, &asked.every].iter().any(|o| given(o).is_some()) {
            return Err("--folder, --done, --verdict-from and --every say how a mail receives: they go with --imap".into());
        }
        let oauth = match given(&asked.oauth) {
            None => None,
            Some(source) => {
                if !source.starts_with("env:") && !source.starts_with("vault:") {
                    return Err("a mail's token request is read from a source, env:HOLON_SECRET_… or vault:…, holding https://id:secret@host/path?scope=…".into());
                }
                let url = self.0.sources.fetch(&Reference::parse(source)?)?;
                let address = oauth::Grant::parse(&url)?.address();
                Some(OAuth { source: source.to_string(), address })
            }
        };
        let mut binding = Binding { oauth: oauth.clone(), ..Binding::default() };
        match smtp {
            Some(smtp) => {
                if !smtp.starts_with("env:") && !smtp.starts_with("vault:") {
                    return Err("a mail's server is read from a source, env:HOLON_SECRET_… or vault:…, holding smtps://user:password@host".into());
                }
                let from = given(&asked.from).unwrap_or_default();
                if holon_core::mail::sender(from).is_none() {
                    return Err(format!("{from:?} isn't a sender: Treasury <treasury@acme.com>, or an address"));
                }
                let to: Vec<String> = asked.to.as_ref().map(To::list).unwrap_or_default().iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
                if to.is_empty() {
                    return Err("a mail sends to the addresses --to says (addresses, *@domain, or * for anyone): it says none".into());
                }
                if let Some(bad) = to.iter().find(|t| !holon_core::mail::is_pattern(t)) {
                    return Err(format!("{bad:?} isn't an address, *@domain or *"));
                }
                let url = self.0.sources.fetch(&Reference::parse(smtp)?)?;
                let server = smtp::Server::parse(&url)?;
                let address = server.address();
                self.with_token(server.wants_token(), &oauth, |token| {
                    let server = smtp::Server::parse(&url).map_err(Failure::Permanent)?;
                    match token {
                    Some(t) => server.with_token(t),
                    None => server,
                }.check()
                })
                .map_err(|f| format!("{address} wasn't logged in to: {}", f.why()))?;
                binding = Binding { smtp: smtp.to_string(), address, from: from.to_string(), to, mailbox: None, oauth: oauth.clone() };
            }
            None if given(&asked.from).is_some() || asked.to.as_ref().is_some_and(|t| !t.list().iter().all(|a| a.trim().is_empty())) => {
                return Err("--from and --to say how a mail sends: they go with --smtp".into());
            }
            None => {}
        }
        if let Some(imap) = imap {
            binding.mailbox = Some(self.mailbox(imap, asked, &oauth)?);
        }
        if oauth.is_some() && !self.logs_in_with_token(&binding) {
            return Err("--oauth logs in where a server's URL names a user and no password (imaps://user@host), and neither does".into());
        }
        let mut bound = self.0.bound.write().unwrap();
        let mut next = bound.clone();
        match for_deployment {
            Some(slug) => next.deployments.entry(slug.to_string()).or_default().insert(name.to_string(), binding.clone()),
            None => next.server.insert(name.to_string(), binding.clone()),
        };
        self.save(&next)?;
        *bound = next;
        Ok(binding)
    }

    /// The mailbox `asked` names, its server logged in to now (with `oauth`'s token when its URL
    /// names no password), its folder selected and the one messages are moved to made.
    fn mailbox(&self, imap: &str, asked: &Asked, oauth: &Option<OAuth>) -> Result<Mailbox, String> {
        if !imap.starts_with("env:") && !imap.starts_with("vault:") {
            return Err("a mailbox is read from a source, env:HOLON_SECRET_… or vault:…, holding imaps://user:password@host".into());
        }
        let folder = given(&asked.folder).unwrap_or("INBOX").to_string();
        let done = given(&asked.done).unwrap_or("Processed").to_string();
        for f in [&folder, &done] {
            if !imap::is_folder_name(f) {
                return Err(format!("{f:?} isn't a folder this host reads: ASCII, without \" \\ & % *"));
            }
        }
        if folder.eq_ignore_ascii_case(&done) {
            return Err(format!("each message is moved out of {folder} once its run is over: --done names another folder"));
        }
        let verdict_from = given(&asked.verdict_from).unwrap_or_default().to_ascii_lowercase();
        if !verdict_from.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b)) {
            return Err(format!("{verdict_from:?} isn't a server's name (the authserv-id its Authentication-Results header writes, mx.example.com)"));
        }
        let every = match given(&asked.every) {
            None => 60,
            Some(e) => folders::seconds_of(e).ok_or_else(|| format!("{e:?} isn't how often: 30s, 1m, 2h"))?,
        };
        if every < EVERY_LEAST {
            return Err(format!("a mailbox is looked at every {EVERY_LEAST}s at most often"));
        }
        let url = self.0.sources.fetch(&Reference::parse(imap)?)?;
        let address = imap::Server::parse(&url)?.address();
        let mut session = self.log_in(&url, oauth).map_err(|f| format!("{address} wasn't logged in to: {}", f.why()))?;
        session.select(&folder).map_err(|f| format!("{folder} wasn't read at {address}: {}", f.why()))?;
        session.create(&done).map_err(|f| format!("{done} wasn't made at {address}: {}", f.why()))?;
        session.logout();
        Ok(Mailbox { imap: imap.to_string(), address, folder, done, verdict_from, every })
    }

    /// `attempt`, given the token for a server that `wants_token` (`oauth`'s, kept or asked for
    /// now) or none for one that doesn't. A login a kept token is refused at drops it and is tried
    /// once more with a new one: a token may be revoked before it runs out. One that wants a token
    /// without `oauth` is refused.
    fn with_token<T>(&self, wants_token: bool, oauth: &Option<OAuth>, attempt: impl Fn(Option<Zeroizing<String>>) -> Result<T, Failure>) -> Result<T, Failure> {
        if !wants_token {
            return attempt(None);
        }
        let o = oauth.as_ref().ok_or_else(|| Failure::Permanent(smtp::NO_TOKEN.into()))?;
        let request = Reference::parse(&o.source).and_then(|r| self.0.sources.fetch(&r)).map_err(Failure::Temporary)?;
        let refused = |outcome: &Result<T, Failure>| matches!(outcome, Err(Failure::Permanent(why)) if why.ends_with("refused the login"));
        let first = attempt(Some(self.0.tokens.token(&request)?));
        if !refused(&first) {
            return first;
        }
        self.0.tokens.forget(&request);
        let again = attempt(Some(self.0.tokens.token(&request)?));
        if refused(&again) {
            self.0.tokens.forget(&request);
        }
        again
    }

    /// Whether one of `binding`'s servers logs in with a token: its URL names a user and no
    /// password.
    fn logs_in_with_token(&self, binding: &Binding) -> bool {
        let wants = |source: &str, smtp: bool| {
            Reference::parse(source).and_then(|r| self.0.sources.fetch(&r)).is_ok_and(|url| if smtp { smtp::Server::parse(&url).is_ok_and(|s| s.wants_token()) } else { imap::Server::parse(&url).is_ok_and(|s| s.wants_token()) })
        };
        (!binding.smtp.is_empty() && wants(&binding.smtp, true)) || binding.mailbox.as_ref().is_some_and(|m| wants(&m.imap, false))
    }

    /// A session with the mailbox `url` names, logged in (with `oauth`'s token when it names no
    /// password).
    fn log_in(&self, url: &str, oauth: &Option<OAuth>) -> Result<imap::Session, Failure> {
        let wants_token = imap::Server::parse(url).map_err(Failure::Permanent)?.wants_token();
        self.with_token(wants_token, oauth, |token| {
            let server = imap::Server::parse(url).map_err(Failure::Permanent)?;
            match token {
                    Some(t) => server.with_token(t),
                    None => server,
                }.open()
        })
    }

    /// A session with the mailbox `binding` receives in, its URL read now, logged in.
    pub(crate) fn open_mailbox(&self, binding: &Binding) -> Result<(imap::Session, Mailbox), String> {
        let mailbox = binding.mailbox.clone().ok_or("it isn't bound to receive now")?;
        let url = Reference::parse(&mailbox.imap).and_then(|r| self.0.sources.fetch(&r))?;
        let session = self.log_in(&url, &binding.oauth).map_err(|f| f.why().to_string())?;
        Ok((session, mailbox))
    }

    /// Unbind `name`, the server's or one deployment's: whether it was bound. Its mail on the way
    /// is given up as it comes to be sent.
    pub(crate) fn unbind(&self, name: &str, for_deployment: Option<&str>) -> Result<bool, String> {
        let mut bound = self.0.bound.write().unwrap();
        let mut next = bound.clone();
        let removed = match for_deployment {
            Some(slug) => {
                let removed = next.deployments.get_mut(slug).and_then(|d| d.remove(name)).is_some();
                if next.deployments.get(slug).is_some_and(BTreeMap::is_empty) {
                    next.deployments.remove(slug);
                }
                removed
            }
            None => next.server.remove(name).is_some(),
        };
        if !removed {
            return Ok(false);
        }
        self.save(&next)?;
        *bound = next;
        Ok(true)
    }

    fn save(&self, bound: &Bound) -> Result<(), String> {
        let Some(file) = &self.0.file else { return Ok(()) };
        let bytes = serde_json::to_vec_pretty(bound).map_err(|e| e.to_string())?;
        crate::sealing::write_private(file, &bytes).map_err(|e| format!("can't write {}: {e}", file.display()))
    }

    /// Why a deployment asking for `manifest` can't run here, if it can't: a mail it sends through
    /// that isn't bound to send, a recipient its text names that the binding doesn't allow, a mail
    /// it receives that isn't bound to receive, or whose mailbox another deployment listens to.
    pub(crate) fn refuses(&self, deployment: &str, manifest: &Manifest) -> Option<String> {
        for item in &manifest.capabilities {
            if manifest.requirements.optional.contains(item) {
                continue;
            }
            if let Some(name) = item.strip_prefix("MailArrival(\"").and_then(|r| r.strip_suffix("\")")) {
                if let Some(why) = self.refuses_listening(deployment, name) {
                    return Some(why);
                }
                continue;
            }
            let Some(named) = item.strip_prefix("MailSend(\"").and_then(|r| r.strip_suffix("\")")) else { continue };
            let (name, to) = named.split_once('/').unwrap_or((named, "*"));
            match self.resolve(deployment, name) {
                Some(b) if !b.smtp.is_empty() && to != "*" && !b.allows(to) => {
                    return Some(format!("the mail {name:?} sends only to {} on this server, and this flow sends to {to}", b.to.join(", ")));
                }
                Some(b) if !b.smtp.is_empty() => {}
                _ => {
                    return Some(format!(
                        "the mail {name:?} isn't bound to send on this server: an admin binds it (holon mail add {name} --smtp env:… --from … --to …, or --for {deployment} for this deployment only)"
                    ));
                }
            }
        }
        None
    }

    /// Why `deployment` can't receive the mail `name`: it isn't bound with `--imap`, or another
    /// deployment listens to its mailbox (a message is moved once: one listener a mailbox).
    fn refuses_listening(&self, deployment: &str, name: &str) -> Option<String> {
        let Some(mailbox) = self.resolve(deployment, name).and_then(|b| b.mailbox) else {
            return Some(format!(
                "the mail {name:?} isn't bound to receive on this server: an admin binds it (holon mail add {name} --imap env:… --verdict-from …, or --for {deployment} for this deployment only)"
            ));
        };
        let deployments = self.0.deployments.get()?;
        let listeners: Vec<(String, Vec<String>)> = deployments
            .lock()
            .unwrap()
            .iter()
            .map(|(slug, d)| (slug.clone(), d.server.as_ref().map(|s| s.mails.iter().map(|m| m.mail.clone()).collect()).unwrap_or_default()))
            .collect();
        self.listened(deployment, &mailbox, &listeners)
    }

    /// Why `deployment` can't listen to `mailbox`: another of `listeners` (each deployment, and
    /// the mails it receives) listens to it, under whatever name.
    fn listened(&self, deployment: &str, mailbox: &Mailbox, listeners: &[(String, Vec<String>)]) -> Option<String> {
        let (slug, _) = listeners.iter().filter(|(slug, _)| slug != deployment).find(|(slug, mails)| {
            mails.iter().any(|m| self.resolve(slug, m).and_then(|b| b.mailbox).is_some_and(|other| other.identity() == mailbox.identity()))
        })?;
        Some(format!("its mailbox ({} {}) is listened to by {slug} already: a message is moved once, so one deployment listens to a mailbox", mailbox.address, mailbox.folder))
    }

    // ── on its way ──

    /// The mails a logged run sent (`mails`: through which, and the message, as it performed
    /// them), spooled before its record is written: its place in the run, the time it began
    /// (`at_ms`, its `Date`) and its id give each its `Message-ID`. One whose mail isn't bound now
    /// is noted and dropped.
    fn spool(&self, slug: &str, run: &str, at_ms: u64, mails: Vec<(String, Value)>, unlogged: bool) -> Vec<Queued> {
        let mut out = Vec::new();
        for (n, (mail, message)) in mails.into_iter().enumerate() {
            let Some((key, binding)) = self.resolve_keyed(slug, &mail).filter(|(_, b)| !b.smtp.is_empty()) else {
                self.note(slug, &mail, format!("didn't mail through {mail}: it isn't bound to send now"));
                continue;
            };
            let domain = binding.sender_address().rsplit_once('@').map(|(_, d)| d.to_string()).unwrap_or_default();
            let id = format!("{run}.{n}@{domain}");
            let Some(bytes) = holon_core::mail::mime(&message, &binding.from, (at_ms / 1000) as i64, &id) else { continue };
            let field = |k: &str| match &message {
                Value::Record(m) => match m.get(k) {
                    Some(Value::List(xs)) => xs.iter().filter_map(|x| if let Value::Text(t) = x { Some(t.clone()) } else { None }).collect(),
                    Some(Value::Text(t)) => vec![t.clone()],
                    _ => vec![],
                },
                _ => vec![],
            };
            let mut to = field("to");
            to.extend(field("cc"));
            let subject = field("subject").pop().unwrap_or_default();
            let spooled = Spooled { slug: slug.to_string(), mail, run: run.to_string(), n, at_ms, to, subject, unlogged };
            let file = self.0.spool.as_ref().map(|dir| dir.join(format!("{slug}.{run}.{n}.mail")));
            if let (Some(dir), Some(file)) = (&self.0.spool, &file) {
                let mut kept = serde_json::to_vec(&spooled).unwrap_or_default();
                kept.push(b'\n');
                kept.extend_from_slice(&bytes);
                if let Err(e) = holon_host::create_private_dir(dir).and_then(|_| crate::sealing::write_private(file, &kept)) {
                    self.note(slug, &spooled.mail, format!("didn't mail {:?} through {}: it couldn't be spooled ({e})", spooled.subject, spooled.mail));
                    continue;
                }
            }
            out.push(Queued { key, spooled, message: bytes, file, tries: 0, not_before: Instant::now() });
        }
        out
    }

    /// Spool a logged run's mails (`entry` is its record, about to be written), before its record
    /// is: what [`hold`](Self::hold) holds. A run that failed or parked sent none.
    pub(crate) fn spool_logged(&self, slug: &str, entry: &LogEntry, mails: Vec<(String, Value)>) -> Vec<Queued> {
        if mails.is_empty() || entry.error.is_some() || !entry.parked.is_empty() {
            return Vec::new();
        }
        self.spool(slug, &entry.run_id, entry.at_ms, mails, false)
    }

    /// Hold the mails spooled for entry `seq`, just written, until it is on disk
    /// ([`release`](Self::release)).
    pub(crate) fn hold(&self, slug: &str, seq: u64, spooled: Vec<Queued>) {
        if !spooled.is_empty() {
            self.0.held.lock().unwrap().entry(slug.to_string()).or_default().extend(spooled.into_iter().map(|q| (seq, q)));
        }
    }

    /// Queue the held mail of `slug`'s entries before `synced`, which are on disk, in its log's
    /// order.
    pub(crate) fn release(&self, slug: &str, synced: u64) {
        let ready: Vec<Queued> = {
            let mut held = self.0.held.lock().unwrap();
            let Some(q) = held.get_mut(slug) else { return };
            let mut ready = Vec::new();
            while let Some((_, item)) = q.pop_front_if(|(seq, _)| *seq < synced) {
                ready.push(item);
            }
            ready
        };
        for item in ready {
            self.queue(item);
        }
    }

    /// Spool and queue an unrecorded run's mails now: its log holds nothing to wait for.
    pub(crate) fn send_unlogged(&self, slug: &str, entry: &LogEntry, mails: Vec<(String, Value)>) {
        if mails.is_empty() || entry.error.is_some() || !entry.parked.is_empty() {
            return;
        }
        for q in self.spool(slug, &entry.run_id, entry.at_ms, mails, true) {
            self.queue(q);
        }
    }

    /// What the spool holds, queued again as the server starts: each mail whose run its
    /// deployment's log holds, finished (`log`: a deployment's entries, none when it isn't
    /// deployed), and each unlogged one. Another's run a crash lost before its record was on
    /// disk: it is removed, never sent.
    pub(crate) fn recover(&self, mut log: impl FnMut(&str) -> Option<Vec<LogEntry>>) {
        let Some(dir) = &self.0.spool else { return };
        let Ok(files) = std::fs::read_dir(dir) else { return };
        let mut logged: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for file in files.flatten().map(|f| f.path()).filter(|p| p.extension().is_some_and(|e| e == "mail")) {
            let Ok(bytes) = std::fs::read(&file) else { continue };
            let Some(end) = bytes.iter().position(|b| *b == b'\n') else { continue };
            let Ok(spooled) = serde_json::from_slice::<Spooled>(&bytes[..end]) else {
                let _ = std::fs::remove_file(&file);
                continue;
            };
            let landed = spooled.unlogged
                || logged
                    .entry(spooled.slug.clone())
                    .or_insert_with(|| log(&spooled.slug).unwrap_or_default().into_iter().filter(|e| e.error.is_none() && e.parked.is_empty()).map(|e| e.run_id).collect())
                    .contains(&spooled.run);
            if !landed {
                let _ = std::fs::remove_file(&file);
                continue;
            }
            let key = self.resolve_keyed(&spooled.slug, &spooled.mail).map_or_else(|| spooled.mail.clone(), |(k, _)| k);
            let message = bytes[end + 1..].to_vec();
            self.queue(Queued { key, spooled, message, file: Some(file), tries: 0, not_before: Instant::now() });
        }
    }

    /// Start sending: each queue's thread, now and as a queue first has mail, its outcomes noted on
    /// `deployments`' timelines.
    pub(crate) fn serve(&self, deployments: Arc<Mutex<BTreeMap<String, Deployment>>>) {
        if self.0.deployments.set(deployments).is_err() {
            return;
        }
        for (key, queue) in self.0.queues.lock().unwrap().iter() {
            self.start(key, Arc::clone(queue));
        }
    }

    fn queue(&self, item: Queued) {
        let mut queues = self.0.queues.lock().unwrap();
        let queue = match queues.get(&item.key) {
            Some(q) => Arc::clone(q),
            None => {
                let q = Arc::new(Queue::default());
                queues.insert(item.key.clone(), Arc::clone(&q));
                if self.0.deployments.get().is_some() {
                    self.start(&item.key, Arc::clone(&q));
                }
                q
            }
        };
        drop(queues);
        queue.items.lock().unwrap().push_back(item);
        queue.ready.notify_one();
    }

    fn start(&self, key: &str, queue: Arc<Queue>) {
        let mails = self.clone();
        let _ = std::thread::Builder::new().name(format!("holon-mail-{key}")).spawn(move || {
            loop {
                let item = {
                    let mut items = queue.items.lock().unwrap();
                    loop {
                        let now = Instant::now();
                        if let Some(i) = items.iter().position(|x| x.not_before <= now) {
                            break items.remove(i).expect("found");
                        }
                        let wait = items.iter().map(|x| x.not_before - now).min().unwrap_or(Duration::from_secs(3600));
                        items = queue.ready.wait_timeout(items, wait).unwrap().0;
                    }
                };
                if let Some(again) = mails.deliver(item) {
                    queue.items.lock().unwrap().push_back(again);
                }
            }
        });
    }

    /// Send `item`: delivered or given up, its spool file removed and its outcome noted; or, after
    /// a temporary failure within the day, given back to try again later.
    fn deliver(&self, mut item: Queued) -> Option<Queued> {
        let s = &item.spooled;
        let sends = self.resolve(&s.slug, &s.mail).filter(|b| !b.smtp.is_empty());
        let outcome = sends.ok_or_else(|| Failure::Permanent(format!("the mail {:?} isn't bound to send now", s.mail))).and_then(|binding| {
            if let Some(r) = s.to.iter().find(|r| !binding.allows(r)) {
                return Err(Failure::Permanent(format!("{r} isn't an address the binding allows now")));
            }
            let url = Reference::parse(&binding.smtp).and_then(|r| self.0.sources.fetch(&r)).map_err(Failure::Temporary)?;
            let wants_token = smtp::Server::parse(&url).map_err(Failure::Permanent)?.wants_token();
            self.with_token(wants_token, &binding.oauth, |token| {
                let server = smtp::Server::parse(&url).map_err(Failure::Permanent)?;
                match token {
                    Some(t) => server.with_token(t),
                    None => server,
                }.send(&binding.sender_address(), &s.to, &item.message)
            })
        });
        let what = format!("{:?} to {} through {}", s.subject, s.to.join(", "), s.mail);
        let (delays, give_up) = self.0.retry;
        let age = Duration::from_millis(firing::started_now().0.saturating_sub(s.at_ms));
        match outcome {
            Ok(()) => self.note(&s.slug, &s.mail, format!("mailed {what}")),
            Err(Failure::Temporary(why)) if age < give_up => {
                if item.tries == 0 {
                    self.note(&s.slug, &s.mail, format!("couldn't mail {what} yet: {why}; trying again"));
                }
                item.not_before = Instant::now() + delays[(item.tries as usize).min(3)];
                item.tries += 1;
                return Some(item);
            }
            Err(f) => {
                eprintln!("holon: {}: gave up mailing {what}: {}", s.slug, f.why());
                self.note(&s.slug, &s.mail, format!("gave up mailing {what}: {}", f.why()));
            }
        }
        if let Some(file) = &item.file {
            let _ = std::fs::remove_file(file);
        }
        None
    }

    pub(crate) fn note(&self, slug: &str, mail: &str, what: String) {
        if let Some(deployments) = self.0.deployments.get() {
            let detail = crate::events::Detail { name: Some(mail.rsplit('/').next().unwrap_or(mail).to_string()), ..Default::default() };
            crate::events::note(deployments, slug, "mail", what, None, detail);
        }
    }
}

/// The mails a run performed, through which, and each message whole: what it spools.
pub(crate) fn of(performed: &[(Capability, Value)]) -> Vec<(String, Value)> {
    performed.iter().filter_map(|(c, p)| if let Capability::MailSend(mail) = c { Some((mail.clone(), p.clone())) } else { None }).collect()
}

/// Why the grant doesn't cover `message`, mailed through `mail`: a recipient it doesn't name, and
/// doesn't say `*` for (`granted` is its `MailSend` names, `ops/approvals@acme.com`).
pub(crate) fn ungranted(mail: &str, message: &Value, granted: &BTreeSet<String>) -> Option<String> {
    if granted.contains(&format!("{mail}/*")) {
        return None;
    }
    let Value::Record(m) = message else { return Some("it isn't a message".into()) };
    let recipients = ["to", "cc"].into_iter().filter_map(|k| m.get(k)).flat_map(|v| if let Value::List(xs) = v { xs.clone() } else { vec![v.clone()] });
    for r in recipients {
        let Value::Text(r) = r else { return Some("a recipient isn't an address".into()) };
        if !granted.iter().any(|g| g.split_once('/').is_some_and(|(m, a)| m == mail && a.eq_ignore_ascii_case(&r))) {
            return Some(format!("{r} isn't a recipient it was granted"));
        }
    }
    None
}

/// Mails past what one run may send: why, if so.
pub(crate) fn past_limit(performed: &[(Capability, Value)]) -> Option<String> {
    let n = performed.iter().filter(|(c, _)| matches!(c, Capability::MailSend(_))).count();
    (n > holon_core::mail::MAX_MAILS_PER_RUN).then(|| format!("a run sends {} mails at most, and this one sent {n}", holon_core::mail::MAX_MAILS_PER_RUN))
}

/// A message as the log keeps it: whole, but each attachment by its size and hash, as a written
/// file is.
pub(crate) fn loggable(payload: &Value) -> Value {
    let Value::Record(m) = payload else { return payload.clone() };
    let Some(Value::Record(files)) = m.get("attachments") else { return payload.clone() };
    let kept = files
        .iter()
        .map(|(name, content)| {
            let bytes: &[u8] = match content {
                Value::Text(t) => t.as_bytes(),
                Value::Bytes(b) => b,
                _ => &[],
            };
            let by_hash = BTreeMap::from([("size".to_string(), Value::Number(bytes.len() as f64)), ("hash".to_string(), Value::Text(blake3::hash(bytes).to_hex().to_string()))]);
            (name.clone(), Value::Record(by_hash))
        })
        .collect();
    let mut m = m.clone();
    m.insert("attachments".into(), Value::Record(kept));
    Value::Record(m)
}

// ── admin ──

fn shown(name: &str, deployment: Option<&str>, b: &Binding) -> serde_json::Value {
    serde_json::json!({ "name": name, "deployment": deployment, "smtp": b.smtp, "address": b.address, "from": b.from, "to": b.to, "mailbox": b.mailbox, "oauth": b.oauth })
}

fn labelled(name: &str, deployment: Option<&str>) -> String {
    deployment.map_or_else(|| name.to_string(), |d| format!("{name} (for {d})"))
}

/// `/admin/mails` (list, bind), `/admin/mails/<name>` (unbind; `?deployment=<slug>` for a
/// deployment's own), or None for any other path. Only an admin gets this far (can.rs); what
/// they do goes in the activity log.
pub(crate) fn route(state: &AppState, request: &Request, by: &str) -> Option<Response> {
    let url = request.url();
    let method = request.method();
    let all = &state.folders.mails;
    if url == "/admin/mails" {
        return Some(match method {
            "GET" => {
                let bound = all.all();
                let server = bound.server.iter().map(|(n, b)| shown(n, None, b));
                let own = bound.deployments.iter().flat_map(|(s, bs)| bs.iter().map(move |(n, b)| shown(n, Some(s), b)));
                Response::json(&server.chain(own).collect::<Vec<_>>())
            }
            "POST" => {
                #[derive(Deserialize)]
                struct Bind {
                    name: String,
                    #[serde(flatten)]
                    asked: Asked,
                    #[serde(default)]
                    deployment: Option<String>,
                }
                let bound = read_body(request)
                    .and_then(|b| serde_json::from_slice::<Bind>(&b).map_err(|e| err(400, format!("invalid body: {e}"))))
                    .map(|b| Bind { deployment: b.deployment.filter(|d| !d.trim().is_empty()), ..b })
                    .and_then(|b| {
                        let m = all.bind(&b.name, &b.asked, b.deployment.as_deref()).map_err(|e| err(400, e))?;
                        Ok((b, m))
                    });
                match bound {
                    Ok((b, m)) => {
                        state.activity.record("bound the mail", &labelled(&b.name, b.deployment.as_deref()), Some(by));
                        if let Some(slug) = &b.deployment {
                            let detail = crate::events::Detail { name: Some(b.name.clone()), ..Default::default() };
                            crate::events::note(&state.deployments, slug, "binding", format!("bound the mail {}", b.name), Some(by), detail);
                        }
                        Response::json(&shown(&b.name, b.deployment.as_deref(), &m))
                    }
                    Err(e) => e,
                }
            }
            _ => Response::empty_404(),
        });
    }
    let name = url.strip_prefix("/admin/mails/")?;
    if method != "DELETE" {
        return Some(Response::empty_404());
    }
    let deployment = request.get_param("deployment").filter(|d| !d.is_empty());
    Some(match all.unbind(name, deployment.as_deref()) {
        Ok(true) => {
            state.activity.record("unbound the mail", &labelled(name, deployment.as_deref()), Some(by));
            if let Some(slug) = &deployment {
                let detail = crate::events::Detail { name: Some(name.to_string()), ..Default::default() };
                crate::events::note(&state.deployments, slug, "binding", format!("unbound the mail {name}"), Some(by), detail);
            }
            Response::empty_204()
        }
        Ok(false) => err(404, format!("no mail {name:?} is bound{}", deployment.map_or(String::new(), |d| format!(" for {d}")))),
        Err(e) => err(500, e),
    })
}

/// `holon mail add NAME [--smtp SOURCE --from SENDER --to LIST] [--imap SOURCE …] [--for SLUG] |
/// list | remove NAME`: bind, show and unbind the mail resources of the server this user runs here
/// (or `--server`), as an admin.
pub(crate) fn cli(args: &[String]) -> i32 {
    let mut server = None;
    let mut for_deployment: Option<String> = None;
    let mut asked = serde_json::Map::new();
    let mut words = Vec::new();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server = args.next().cloned(),
            "--for" => for_deployment = args.next().cloned(),
            "--smtp" | "--from" | "--to" | "--imap" | "--folder" | "--done" | "--verdict-from" | "--every" | "--oauth" => {
                let Some(v) = args.next() else {
                    eprintln!("holon mail: {a} takes a value\n\n{MAIL_USAGE}");
                    return 2;
                };
                asked.insert(a[2..].replace('-', "_"), serde_json::Value::String(v.clone()));
            }
            "--help" | "-h" => {
                println!("{MAIL_USAGE}");
                return 0;
            }
            _ if !a.starts_with('-') => words.push(a.as_str()),
            _ => {
                eprintln!("holon mail: no option {a:?}\n\n{MAIL_USAGE}");
                return 2;
            }
        }
    }
    let (method, path, body) = match words.as_slice() {
        ["list"] => ("GET", "/admin/mails".to_string(), None),
        ["add", name] => {
            if !asked.contains_key("smtp") && !asked.contains_key("imap") {
                eprintln!("holon mail add: it takes --smtp SOURCE (with --from SENDER and --to LIST) to send, --imap SOURCE to receive, or both\n\n{MAIL_USAGE}");
                return 2;
            }
            asked.insert("name".into(), serde_json::Value::String(name.to_string()));
            asked.insert("deployment".into(), serde_json::json!(for_deployment));
            ("POST", "/admin/mails".to_string(), Some(serde_json::Value::Object(asked)))
        }
        ["remove", name] => {
            let only = for_deployment.as_ref().map_or(String::new(), |d| format!("?deployment={}", crate::mcp::seg(d)));
            ("DELETE", format!("/admin/mails/{}{only}", crate::mcp::seg(name)), None)
        }
        _ => {
            eprintln!("{MAIL_USAGE}");
            return 2;
        }
    };
    let (server, answer) = match local::admin_call("holon mail", server.as_deref(), method, &path, body.as_ref()) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let line = |m: &serde_json::Value| {
        let only = m["deployment"].as_str().map_or(String::new(), |s| format!(", for {s} only"));
        let to: Vec<&str> = m["to"].as_array().into_iter().flatten().filter_map(|t| t.as_str()).collect();
        let mut said = vec![m["name"].as_str().unwrap_or("").to_string()];
        if m["smtp"].as_str().is_some_and(|s| !s.is_empty()) {
            said.push(format!("sends through {} (from {})  as {}  to {}", m["address"].as_str().unwrap_or(""), m["smtp"].as_str().unwrap_or(""), m["from"].as_str().unwrap_or(""), to.join(", ")));
        }
        let b = &m["mailbox"];
        if let Some(imap) = b["imap"].as_str() {
            let verdict = b["verdict_from"].as_str().map_or("nobody's verdict".to_string(), |v| format!("{v}'s verdict"));
            let every = folders::span_text(b["every"].as_u64().unwrap_or(60));
            said.push(format!("receives in {} {} (from {imap}), moved to {}  {verdict}  every {every}", b["address"].as_str().unwrap_or(""), b["folder"].as_str().unwrap_or(""), b["done"].as_str().unwrap_or("")));
        }
        if let Some(source) = m["oauth"]["source"].as_str() {
            said.push(format!("logs in with OAuth ({} from {source})", m["oauth"]["address"].as_str().unwrap_or("")));
        }
        format!("{}{only}", said.join("  "))
    };
    match words[0] {
        "list" => {
            let all: Vec<serde_json::Value> = serde_json::from_str(&answer).unwrap_or_default();
            if all.is_empty() {
                println!("no mails are bound on {server}: holon mail add NAME --smtp env:… --from … --to … (or --imap env:…) binds one");
            }
            for m in &all {
                println!("{}", line(m));
            }
        }
        "add" => println!("bound {}", line(&serde_json::from_str(&answer).unwrap_or_default())),
        _ => println!("unbound {}{}: its mail on the way is given up", words[1], for_deployment.as_ref().map_or(String::new(), |d| format!(" (for {d})"))),
    }
    0
}

const MAIL_USAGE: &str = "usage: holon mail add NAME [--smtp SOURCE --from SENDER --to LIST]
                             [--imap SOURCE [--folder F] [--done F] [--verdict-from SERVER] [--every T]]
                             [--oauth SOURCE] [--for SLUG]
       holon mail list
       holon mail remove NAME [--for SLUG]
       (each with [--server PORT|URL])

  Binds a name a flow imports (import ops from \"mail:ops\") to an SMTP server it sends
  through, a mailbox whose messages start its runs (on.mail(ops, …)), or both. A flow sends only
  to the recipients it was granted, within the binding's; it never sees a server or its login,
  and never chooses the sender. Admins.

  add NAME                 bind NAME, logging in to each server now
    --smtp SOURCE          where the server reads its URL, as it reads a secret:
                           env:HOLON_SECRET_OPS_SMTP (its environment or --dotenv file) or
                           vault:<mount>/<path>#<field>. The URL is never stored, shown or
                           logged: smtps://user:password@smtp.example.com (TLS, port 465), or
                           smtp://user:password@smtp.example.com:587 (STARTTLS, required);
                           plain smtp:// only to a relay on this machine
    --from SENDER          who the mail is from: \"Treasury <treasury@acme.com>\"
    --to LIST              every address it may send to, with commas: addresses, *@domain,
                           or * for anyone
    --imap SOURCE          where the server reads the mailbox's URL, as --smtp:
                           imaps://user:password@imap.example.com (TLS, port 993), or
                           imap://user:password@imap.example.com:143 (STARTTLS, required)
    --folder F             the folder read (INBOX)
    --done F               where each message is moved once its run is over (Processed),
                           made now if it isn't there
    --verdict-from SERVER  whose Authentication-Results verdict counts: the name the
                           mailbox's own server writes there (mx.google.com). Without it, no
                           message is authenticated
    --every T              how often the folder is looked at: 1m, 30s (10s at least)
    --oauth SOURCE         where the server reads the token request, for a server whose URL
                           names a user and no password (imaps://user@outlook.office365.com):
                           https://id:secret@login.microsoftonline.com/<tenant>/oauth2/v2.0/token
                           ?scope=… (client credentials) or ?refresh_token=… (a refresh token).
                           The host asks for a token and logs in with it (XOAUTH2)
    --for SLUG             bind it for that deployment only
  list                     the names bound, and where each sends
  remove NAME              unbind it (--for SLUG: that deployment's). The mail on its way is
                           given up, and none goes through it until it is bound again
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon mail add ops --smtp env:HOLON_SECRET_OPS_SMTP --from \"Treasury <treasury@acme.com>\" --to \"*@acme.com,treasury@bank.example\"
  holon mail add remit --imap env:HOLON_SECRET_REMIT_IMAP --done Processed --verdict-from mx.acme.com
  holon mail add support --imap env:HOLON_SECRET_SUPPORT_IMAP --oauth env:HOLON_SECRET_SUPPORT_OAUTH
  holon mail list

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  with an API token in HOLON_TOKEN.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smtp::sink;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("holon-mails-{tag}-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sources(url: &str) -> Sources {
        sources_of(&[("HOLON_SECRET_OPS_SMTP", url)])
    }

    fn sources_of(vars: &[(&str, &str)]) -> Sources {
        let dir = temp("env");
        let env = dir.join(".env");
        std::fs::write(&env, vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect::<String>()).unwrap();
        Sources::new(Some(env)).unwrap()
    }

    /// A binding that sends through `smtp`, as `--smtp`, `--from` and `--to` ask.
    fn send(smtp: &str, from: &str, to: &str) -> Asked {
        Asked { smtp: Some(smtp.into()), from: Some(from.into()), to: Some(To::One(to.into())), ..Default::default() }
    }

    fn message(to: &[&str], subject: &str) -> Value {
        let list = Value::List(to.iter().map(|t| Value::Text(t.to_string())).collect());
        Value::Record(BTreeMap::from([("to".into(), list), ("subject".into(), Value::Text(subject.into())), ("text".into(), Value::Text("hello".into()))]))
    }

    #[test]
    fn a_mail_is_bound_by_an_admin_logged_in_to_and_held_to_its_recipients() {
        let s = sink::start("PLAIN");
        let dir = temp("bind");
        let file = dir.join("mails.json");
        let mails = Mails::open(file.to_str().unwrap()).unwrap().with_sources(sources(&format!("smtp://ops:pw@127.0.0.1:{}", s.port)));
        assert!(mails.bind("ops", &send("smtp://u:p@x", "t@acme.com", "*"), None).unwrap_err().contains("read from a source"));
        assert!(mails.bind("ops", &send("env:HOLON_SECRET_OPS_SMTP", "Treasury", "*"), None).unwrap_err().contains("isn't a sender"));
        assert!(mails.bind("ops", &send("env:HOLON_SECRET_OPS_SMTP", "t@acme.com", ""), None).unwrap_err().contains("says none"));
        assert!(mails.bind("ops", &send("env:HOLON_SECRET_OPS_SMTP", "t@acme.com", "acme.com"), None).unwrap_err().contains("isn't an address"));
        assert!(mails.bind("ops", &send("env:HOLON_SECRET_MISSING", "t@acme.com", "*"), None).unwrap_err().contains("isn't set"));
        assert!(mails.bind("ops", &Asked::default(), None).unwrap_err().contains("says neither"));
        assert!(mails.bind("ops", &Asked { from: Some("t@acme.com".into()), imap: Some("env:X".into()), ..Default::default() }, None).unwrap_err().contains("go with --smtp"));
        let b = mails.bind("ops", &send("env:HOLON_SECRET_OPS_SMTP", "Treasury <treasury@acme.com>", "*@acme.com, treasury@bank.example"), None).unwrap();
        assert_eq!(b.address, format!("smtp://127.0.0.1:{}", s.port));
        // Kept without the URL or its login, and read back.
        let kept = std::fs::read_to_string(&file).unwrap();
        assert!(!kept.contains("pw") && kept.contains("env:HOLON_SECRET_OPS_SMTP"), "{kept}");
        assert_eq!(Mails::open(file.to_str().unwrap()).unwrap().resolve("app", "ops"), Some(b.clone()));
        let manifest = |caps: &[&str]| Manifest { capabilities: caps.iter().map(|c| c.to_string()).collect(), requirements: Default::default() };
        assert_eq!(mails.refuses("app", &manifest(&[r#"MailSend("ops/approvals@acme.com")"#, r#"MailSend("ops/*")"#])), None);
        assert!(mails.refuses("app", &manifest(&[r#"MailSend("ops/someone@gmail.com")"#])).unwrap().contains("sends only to *@acme.com, treasury@bank.example"));
        assert!(mails.refuses("app", &manifest(&[r#"MailSend("alerts/*")"#])).unwrap().contains("the mail \"alerts\" isn't bound to send"));
        assert!(mails.refuses("app", &manifest(&[r#"MailArrival("ops")"#])).unwrap().contains("the mail \"ops\" isn't bound to receive"));
        assert!(mails.unbind("ops", None).unwrap());
        assert!(!mails.unbind("ops", None).unwrap());
    }

    /// Bound to receive: the mailbox logged in to, its folder read and `--done` made; kept without
    /// its URL; a flow that receives through it deploys, one that sends through it doesn't; and a
    /// mailbox one deployment listens to is refused to another, under any name.
    #[test]
    fn a_mail_bound_with_imap_receives_and_one_deployment_listens_to_a_mailbox() {
        let imap = crate::imap::stand_in::start("remit", "s3cret", true);
        let dir = temp("imap");
        let file = dir.join("mails.json");
        let url = format!("imap://remit:s3cret@127.0.0.1:{}", imap.port);
        let wrong = format!("imap://remit:nope@127.0.0.1:{}", imap.port);
        let mails = Mails::open(file.to_str().unwrap()).unwrap().with_sources(sources_of(&[("HOLON_SECRET_REMIT_IMAP", &url), ("HOLON_SECRET_WRONG_IMAP", &wrong)]));
        let receive = |imap: &str, more: fn(Asked) -> Asked| more(Asked { imap: Some(imap.into()), ..Default::default() });
        for (asked, said) in [
            (receive("imaps://u:p@x", |a| a), "read from a source"),
            (receive("env:HOLON_SECRET_WRONG_IMAP", |a| a), "refused the login"),
            (receive("env:HOLON_SECRET_REMIT_IMAP", |a| Asked { folder: Some("Elsewhere".into()), ..a }), "Elsewhere wasn't read"),
            (receive("env:HOLON_SECRET_REMIT_IMAP", |a| Asked { done: Some("INBOX".into()), ..a }), "names another folder"),
            (receive("env:HOLON_SECRET_REMIT_IMAP", |a| Asked { done: Some("Réglé".into()), ..a }), "isn't a folder"),
            (receive("env:HOLON_SECRET_REMIT_IMAP", |a| Asked { every: Some("5s".into()), ..a }), "10s at most often"),
            (receive("env:HOLON_SECRET_REMIT_IMAP", |a| Asked { verdict_from: Some("mx acme".into()), ..a }), "isn't a server's name"),
            (Asked { folder: Some("INBOX".into()), ..send("env:HOLON_SECRET_OPS_SMTP", "t@acme.com", "*") }, "go with --imap"),
        ] {
            let e = mails.bind("remit", &asked, None).unwrap_err();
            assert!(e.contains(said) && !e.contains("s3cret") && !e.contains("nope"), "{said}: {e}");
        }
        let b = mails.bind("remit", &receive("env:HOLON_SECRET_REMIT_IMAP", |a| Asked { verdict_from: Some("MX.acme.com".into()), every: Some("30s".into()), ..a }), None).unwrap();
        let mailbox = b.mailbox.clone().unwrap();
        assert_eq!((b.smtp.as_str(), mailbox.folder.as_str(), mailbox.done.as_str(), mailbox.verdict_from.as_str(), mailbox.every), ("", "INBOX", "Processed", "mx.acme.com", 30));
        assert_eq!(mailbox.address, format!("imap://127.0.0.1:{}", imap.port));
        assert!(imap.folders.lock().unwrap().folders.contains_key("Processed"), "--done made");
        let kept = std::fs::read_to_string(&file).unwrap();
        assert!(!kept.contains("s3cret") && kept.contains("env:HOLON_SECRET_REMIT_IMAP"), "{kept}");
        assert_eq!(Mails::open(file.to_str().unwrap()).unwrap().resolve("app", "remit"), Some(b.clone()));
        let manifest = |caps: &[&str]| Manifest { capabilities: caps.iter().map(|c| c.to_string()).collect(), requirements: Default::default() };
        assert_eq!(mails.refuses("app", &manifest(&[r#"MailArrival("remit")"#])), None);
        assert!(mails.refuses("app", &manifest(&[r#"MailSend("remit/*")"#])).unwrap().contains("isn't bound to send"));
        // Listened to by `app`: another deployment, under the same name or another, is refused.
        mails.bind("payments", &receive("env:HOLON_SECRET_REMIT_IMAP", |a| a), Some("other")).unwrap();
        let listening = [("app".to_string(), vec!["remit".to_string()])];
        assert_eq!(mails.listened("app", &mailbox, &listening), None, "its own redeploy");
        assert!(mails.listened("next", &mailbox, &listening).unwrap().contains("listened to by app already"));
        let theirs = mails.resolve("other", "payments").unwrap().mailbox.unwrap();
        assert!(mails.listened("other", &theirs, &listening).unwrap().contains("listened to by app already"));
    }

    /// A mailbox whose URL names a user and no password logs in with the token `--oauth` asks for,
    /// asked once and kept; refused without `--oauth`, with a client the endpoint refuses, and with
    /// `--oauth` where every URL holds a password. Neither the file nor an error holds a secret.
    #[test]
    fn a_mail_bound_with_oauth_logs_in_with_a_token() {
        let endpoint = crate::oauth::stand_in::start("app", "s3cret", "t0ken");
        let imap = crate::imap::stand_in::start_with_token("support@acme.com", "t0ken", true);
        let dir = temp("oauth");
        let file = dir.join("mails.json");
        let mailbox = format!("imap://support%40acme.com@127.0.0.1:{}", imap.port);
        let passwords = crate::imap::stand_in::start("support@acme.com", "pw", true);
        let with_password = format!("imap://support%40acme.com:pw@127.0.0.1:{}", passwords.port);
        let mails = Mails::open(file.to_str().unwrap()).unwrap().with_sources(sources_of(&[
            ("HOLON_SECRET_SUPPORT_IMAP", &mailbox),
            ("HOLON_SECRET_PASSWORD_IMAP", &with_password),
            ("HOLON_SECRET_SUPPORT_OAUTH", &endpoint.url("app", "s3cret", "scope=mail")),
            ("HOLON_SECRET_WRONG_OAUTH", &endpoint.url("app", "n0pe", "scope=mail")),
        ]));
        let receive = |imap: &str, oauth: Option<&str>| Asked { imap: Some(imap.into()), oauth: oauth.map(str::to_string), ..Default::default() };
        for (asked, said) in [
            (receive("env:HOLON_SECRET_SUPPORT_IMAP", None), "no --oauth to log in with"),
            (receive("env:HOLON_SECRET_SUPPORT_IMAP", Some("https://app:s3cret@login.example.com/token?scope=mail")), "read from a source"),
            (receive("env:HOLON_SECRET_SUPPORT_IMAP", Some("env:HOLON_SECRET_WRONG_OAUTH")), "invalid_client"),
            (receive("env:HOLON_SECRET_PASSWORD_IMAP", Some("env:HOLON_SECRET_SUPPORT_OAUTH")), "and neither does"),
        ] {
            let e = mails.bind("support", &asked, None).unwrap_err();
            assert!(e.contains(said) && !e.contains("s3cret") && !e.contains("n0pe") && !e.contains("t0ken"), "{said}: {e}");
        }
        let before = endpoint.asked.lock().unwrap().len();
        let b = mails.bind("support", &receive("env:HOLON_SECRET_SUPPORT_IMAP", Some("env:HOLON_SECRET_SUPPORT_OAUTH")), None).unwrap();
        assert_eq!(b.oauth, Some(OAuth { source: "env:HOLON_SECRET_SUPPORT_OAUTH".into(), address: format!("http://127.0.0.1:{}/token", endpoint.port) }));
        assert_eq!(imap.folders.lock().unwrap().logins, 1);
        let (session, _) = mails.open_mailbox(&b).unwrap();
        session.logout();
        assert_eq!(imap.folders.lock().unwrap().logins, 2);
        assert_eq!(endpoint.asked.lock().unwrap().len(), before + 1, "the token kept");
        let kept = std::fs::read_to_string(&file).unwrap();
        assert!(!kept.contains("s3cret") && !kept.contains("t0ken") && kept.contains("env:HOLON_SECRET_SUPPORT_OAUTH"), "{kept}");
    }

    #[test]
    fn a_grant_covers_the_recipients_it_names_or_any_for_a_star() {
        let granted: BTreeSet<String> = ["ops/approvals@acme.com".to_string()].into();
        assert_eq!(ungranted("ops", &message(&["Approvals@acme.com"], "s"), &granted), None);
        assert!(ungranted("ops", &message(&["approvals@acme.com", "x@acme.com"], "s"), &granted).unwrap().contains("x@acme.com"));
        assert!(ungranted("alerts", &message(&["approvals@acme.com"], "s"), &granted).is_some());
        assert_eq!(ungranted("ops", &message(&["anyone@acme.com"], "s"), &["ops/*".to_string()].into()), None);
        let many: Vec<(Capability, Value)> = (0..101).map(|_| (Capability::MailSend("ops".into()), Value::Boolean(true))).collect();
        assert!(past_limit(&many).unwrap().contains("100 mails at most"));
        assert_eq!(past_limit(&many[..100]), None);
    }

    #[test]
    fn the_log_keeps_attachments_by_size_and_hash() {
        let Value::Record(mut m) = message(&["a@acme.com"], "s") else { panic!() };
        m.insert("attachments".into(), Value::Record(BTreeMap::from([("a.csv".to_string(), Value::Text("x,y".into()))])));
        let Value::Record(kept) = loggable(&Value::Record(m)) else { panic!() };
        let expected = Value::Record(BTreeMap::from([("a.csv".to_string(), Value::Record(BTreeMap::from([("size".into(), Value::Number(3.0)), ("hash".into(), Value::Text(blake3::hash(b"x,y").to_hex().to_string()))])))]));
        assert_eq!(kept.get("attachments"), Some(&expected));
        assert_eq!(kept.get("text"), Some(&Value::Text("hello".into())));
    }

    fn entry(run: &str) -> LogEntry {
        LogEntry { run_id: run.into(), at_ms: firing::started_now().0, ..Default::default() }
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < until, "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn mail_waits_for_its_record_then_goes_in_order_and_a_deferred_one_is_tried_again() {
        let s = sink::start("");
        let dir = temp("send");
        let mails = Mails::open(dir.join("mails.json").to_str().unwrap())
            .unwrap()
            .with_sources(sources(&format!("smtp://127.0.0.1:{}", s.port)))
            .with_retry([Duration::from_millis(50); 4], Duration::from_secs(5));
        mails.bind("ops", &send("env:HOLON_SECRET_OPS_SMTP", "Treasury <treasury@acme.com>", "*@acme.com"), None).unwrap();
        mails.serve(Arc::new(Mutex::new(BTreeMap::new())));
        mails.hold("app", 7, mails.spool_logged("app", &entry("r1"), vec![("ops".into(), message(&["a@acme.com"], "first")), ("ops".into(), message(&["b@acme.com"], "second"))]));
        // Spooled, not sent: its record isn't on disk.
        assert_eq!(std::fs::read_dir(dir.join("mail-spool")).unwrap().count(), 2);
        std::thread::sleep(Duration::from_millis(100));
        assert!(s.got.lock().unwrap().is_empty());
        mails.release("app", 7);
        std::thread::sleep(Duration::from_millis(100));
        assert!(s.got.lock().unwrap().is_empty(), "entry 7 isn't before 7");
        mails.release("app", 8);
        wait_for("both sent", || s.got.lock().unwrap().len() == 2);
        let got = s.got.lock().unwrap().clone();
        assert!(got[0].data.contains("Subject: first") && got[1].data.contains("Subject: second"));
        assert!(got[0].data.contains("Message-ID: <r1.0@acme.com>") && got[1].data.contains("Message-ID: <r1.1@acme.com>"));
        assert_eq!(got[0].from, "treasury@acme.com");
        wait_for("spool emptied", || std::fs::read_dir(dir.join("mail-spool")).unwrap().count() == 0);
        // A failed run's, or a parked one's: nothing.
        mails.hold("app", 9, mails.spool_logged("app", &LogEntry { error: Some("x".into()), ..entry("r2") }, vec![("ops".into(), message(&["a@acme.com"], "failed"))]));
        assert_eq!(std::fs::read_dir(dir.join("mail-spool")).unwrap().count(), 0);
        // A recipient put off is tried again; one refused is given up, the rest still going.
        mails.send_unlogged("app", &entry("r3"), vec![("ops".into(), message(&["later@acme.com"], "deferred")), ("ops".into(), message(&["reject@acme.com"], "refused")), ("ops".into(), message(&["c@acme.com"], "third"))]);
        wait_for("third sent past the others", || s.got.lock().unwrap().iter().any(|g| g.data.contains("Subject: third")));
        wait_for("only the deferred one left", || std::fs::read_dir(dir.join("mail-spool")).unwrap().count() == 1);
    }

    #[test]
    fn a_restart_sends_what_the_spool_holds_whose_run_landed() {
        let dir = temp("recover");
        let spool = dir.join("mail-spool");
        let mails = Mails::open(dir.join("mails.json").to_str().unwrap()).unwrap().with_sources(sources("smtp://127.0.0.1:1"));
        // Bound to a server that isn't there: spooled, and kept.
        mails.0.bound.write().unwrap().server.insert("ops".into(), Binding { smtp: "env:HOLON_SECRET_OPS_SMTP".into(), from: "t@acme.com".into(), to: vec!["*".into()], ..Default::default() });
        mails.hold("app", 1, mails.spool_logged("app", &entry("landed"), vec![("ops".into(), message(&["a@acme.com"], "landed"))]));
        mails.hold("app", 2, mails.spool_logged("app", &entry("lost"), vec![("ops".into(), message(&["a@acme.com"], "lost"))]));
        mails.send_unlogged("app", &entry("unlogged"), vec![("ops".into(), message(&["a@acme.com"], "unlogged"))]);
        assert_eq!(std::fs::read_dir(&spool).unwrap().count(), 3);
        // The log holds the first run only: the second's record a crash lost.
        let again = Mails::open(dir.join("mails.json").to_str().unwrap()).unwrap();
        again.recover(|slug| (slug == "app").then(|| vec![LogEntry { run_id: "landed".into(), ..Default::default() }]));
        let mut left: Vec<String> = std::fs::read_dir(&spool).unwrap().flatten().map(|f| f.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["app.landed.0.mail", "app.unlogged.0.mail"]);
        let queued: usize = again.0.queues.lock().unwrap().values().map(|q| q.items.lock().unwrap().len()).sum();
        assert_eq!(queued, 2);
    }
}
