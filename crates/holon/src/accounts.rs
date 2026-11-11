//! Who uses this server, and how they get in: its principals, kept in `--accounts` (default
//! `.holon-accounts.json` beside the other state, owner-only).
//!
//! - **First run.** A server without accounts answers nothing but its setup: it prints a one-time
//!   setup code at startup (or takes `HOLON_SETUP_CODE`), and whoever enters it in the editor
//!   creates the first admin. Nobody else on the network can claim it first.
//! - **Principals** have a name, unique across kinds, a kind and a role (`viewer`: reads;
//!   `editor`: also saves, deploys, runs; `admin`: also writes secrets and manages the server;
//!   `agent`: reads, proposes and calls, access.rs). The kind says who it is and which roles it
//!   may hold (docs/L4s/access-and-approvals-L4.md §3, §5.1):
//!   - a **person** (viewer, editor or admin) signs in with a password, kept as an argon2id hash,
//!     through an identity provider, or through the run file. Signing in gives a session token
//!     (`hs_…`, for 7 days); a few wrong passwords in a row lock that name for a minute;
//!   - an **agent** (agent or viewer) and a **service** (viewer or editor) sign in with their own
//!     API tokens (`ht_…`), and have an owner, the person answerable for them.
//! - **Invites.** An admin adds a person by name and role and gets a one-time link (`hi_…`, for
//!   7 days): its holder sets their own password and is signed in. A reset is a new link.
//! - **API tokens** belong to an agent or a service: made by an admin or its owner, shown once,
//!   revoked any time. Disabling the principal stops its tokens.
//! - **Signing in through an identity provider** (oidc.rs), optional: an admin sets the issuer
//!   and client id; someone signing in is matched to the account with their email.
//!
//! Only hashes of tokens are kept, never the tokens; passwords only as argon2id hashes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::{Deserialize, Serialize};

use crate::access::Role;
use crate::deploy::epoch_now;

const SESSION_SECONDS: u64 = 7 * 24 * 3600;
const INVITE_SECONDS: u64 = 7 * 24 * 3600;
/// The shortest password accepted.
pub(crate) const MIN_PASSWORD: usize = 10;
/// Wrong passwords in a row before a name is locked, and for how long.
const MAX_FAILURES: u32 = 5;
const LOCKED_FOR: Duration = Duration::from_secs(60);

/// What a principal is (docs/L4s/access-and-approvals-L4.md §3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    #[default]
    Person,
    Agent,
    Service,
}

impl Kind {
    /// Whether it may hold `role` (the design's §5.1): a person any but agent, a service viewer
    /// or editor, an agent viewer or agent.
    pub(crate) fn may_hold(self, role: Role) -> bool {
        match self {
            Kind::Person => role != Role::Agent,
            Kind::Service => matches!(role, Role::Viewer | Role::Editor),
            Kind::Agent => matches!(role, Role::Viewer | Role::Agent),
        }
    }

    /// As a sentence says it: "an agent".
    pub(crate) fn said(self) -> &'static str {
        match self {
            Kind::Person => "a person",
            Kind::Agent => "an agent",
            Kind::Service => "a service",
        }
    }
}

/// Why `kind` may not hold `role`, if it may not.
fn held(kind: Kind, role: Role) -> Result<(), String> {
    if kind.may_hold(role) {
        return Ok(());
    }
    Err(match kind {
        Kind::Person => "a person is a viewer, an editor or an admin: an agent is a principal of its own",
        Kind::Service => "a service is a viewer or an editor",
        Kind::Agent => "an agent is an agent or a viewer",
    }
    .into())
}

/// Why `owner` may not own an agent or a service, if it may not: it must be a person here who can
/// sign in, or the OS user who runs the server (`local`, local.rs).
fn may_own(book: &Book, owner: &str, local: Option<&str>) -> Result<(), String> {
    if local == Some(owner) {
        return Ok(());
    }
    match book.users.iter().find(|u| u.name == owner) {
        Some(p) if p.kind != Kind::Person => Err(format!("{owner} is {}: an owner is a person", p.kind.said())),
        Some(p) if p.disabled => Err(format!("{owner} is disabled: an owner is a person who can sign in")),
        Some(_) => Ok(()),
        None => Err(format!("there is no person {owner:?} here to own it")),
    }
}

/// Whether `book` has no person: a server not set up yet.
fn no_person(book: &Book) -> bool {
    !book.users.iter().any(|u| u.kind == Kind::Person)
}

/// A person, an agent or a service.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub(crate) struct Principal {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) kind: Kind,
    pub(crate) role: Role,
    /// How it is shown, when not by its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) display: Option<String>,
    /// A person's argon2id hash; absent until an invite is accepted, and for an agent or a service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    /// Who a person is at the identity provider, when signing in through one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) email: Option<String>,
    #[serde(default)]
    pub(crate) disabled: bool,
    pub(crate) created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) created_by: Option<String>,
    /// An agent's or a service's: the person answerable for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner: Option<String>,
    /// What the editor keeps for them (its home tab's panes, whether the welcome shows on
    /// start): opaque here, so it follows them to any browser.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    prefs: serde_json::Value,
}

impl Principal {
    /// A new principal, made now by `by` (none: the first admin), with no password, email or owner.
    fn new(name: &str, kind: Kind, role: Role, by: Option<&str>) -> Principal {
        Principal {
            name: name.into(),
            kind,
            role,
            display: None,
            password: None,
            email: None,
            disabled: false,
            created_at: epoch_now(),
            created_by: by.map(String::from),
            owner: None,
            prefs: serde_json::Value::Null,
        }
    }
}

/// The most a person's preferences may take, as JSON.
pub(crate) const MAX_PREFS_BYTES: usize = 16 * 1024;

#[derive(Clone, Serialize, Deserialize)]
struct Invite {
    hash: String,
    name: String,
    expires_at: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct Session {
    hash: String,
    user: String,
    expires_at: u64,
}

/// An agent's or a service's token, kept as its hash.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub(crate) struct ApiToken {
    pub(crate) id: String,
    /// The agent or service it signs in as.
    pub(crate) principal: String,
    /// What it is for ("CI on GitHub").
    pub(crate) label: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    hash: String,
    pub(crate) created_at: u64,
    pub(crate) created_by: String,
}

/// Signing in through an identity provider, as an admin set it.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub(crate) struct OidcSettings {
    pub(crate) issuer: String,
    pub(crate) client_id: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Book {
    /// Every principal, of every kind.
    #[serde(default)]
    users: Vec<Principal>,
    #[serde(default)]
    invites: Vec<Invite>,
    #[serde(default)]
    sessions: Vec<Session>,
    /// The agents' and services' API tokens. A file's `tokens`, from when a token had a role of
    /// its own, are not read.
    #[serde(default)]
    credentials: Vec<ApiToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oidc: Option<OidcSettings>,
}

/// Who a token is: a principal's name (or the OS user's, `local:<user>`), its kind and its role.
#[derive(Debug, PartialEq, Clone)]
pub(crate) struct Who {
    pub(crate) name: String,
    pub(crate) kind: Kind,
    pub(crate) role: Role,
}

/// A name's sign-ins: wrong passwords since its last lock, when it was locked, and how many are
/// being checked now (each counts as wrong until it is known not to be).
#[derive(Default)]
struct Tries {
    wrong: u32,
    locked: Option<Instant>,
    checking: u32,
    /// When it was last tried.
    last: Option<Instant>,
}

/// Where a password change's tries are counted from, in place of a client's address: by name,
/// apart from signing in.
const CHANGING: &str = "(changing the password)";

/// How many names' tries are kept before the old ones are forgotten.
const TRIES_KEPT: usize = 4096;

/// Forget the tries no longer locking, checking or recent (a minute), once there are more than
/// `TRIES_KEPT`: sign-ins under ever new names can't grow the map for good.
fn forget_old_tries(failures: &mut HashMap<(String, String), Tries>) {
    if failures.len() > TRIES_KEPT {
        failures.retain(|_, t| t.checking > 0 || t.locked.is_some_and(|at| at.elapsed() < LOCKED_FOR) || t.last.is_some_and(|at| at.elapsed() < LOCKED_FOR));
    }
}

pub(crate) struct Accounts {
    path: Option<PathBuf>,
    book: Mutex<Book>,
    /// The one-time code that creates the first admin, while there is none.
    setup_code: Mutex<Option<String>>,
    /// Each name's sign-ins from each address (`sign_in_from`).
    failures: Mutex<HashMap<(String, String), Tries>>,
    /// Wrong setup codes: as a name's wrong passwords, `MAX_FAILURES` then locked for
    /// `LOCKED_FOR`, so the code (about 40 bits) can't be guessed at speed while the server waits
    /// for its first admin.
    setup_tries: Mutex<Tries>,
    /// The identity provider, discovered from `book.oidc` (oidc.rs).
    oidc: RwLock<Option<crate::oidc::Oidc>>,
    /// The local token's hash and who it is (local.rs): made when the server starts, good as that
    /// OS user's admin until it stops, never stored.
    local: Mutex<Option<(String, String)>>,
}

/// A new secret token: `prefix` and 32 random bytes, in hex.
fn new_token(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random source");
    format!("{prefix}{}", crate::sealing::to_hex(&bytes))
}

fn hash_of(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

/// Constant time, as tokens are compared everywhere (access.rs).
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn valid_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-@".contains(&b));
    if !ok || name == "admin-token" {
        return Err("a name is letters, digits and . _ - @, at most 64".into());
    }
    Ok(())
}

fn check_password(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD {
        return Err(format!("a password needs at least {MIN_PASSWORD} characters"));
    }
    Ok(())
}

/// How many passwords are hashed or checked at once, at most: each takes argon2's memory (19 MiB
/// by default) and a core for tens of milliseconds, so sign-ins sent by the thousand at once
/// would take the machine's memory. The rest wait their turn, up to `CHECK_WAIT`, then are told
/// the server is busy. The wait is long: the cap is what bounds the memory, and a sign-in that
/// waits its turn is better than one turned away on a busy machine (two seconds turned away
/// sign-ins on a loaded two-core CI runner, where a check in a debug build takes longer).
/// What waits is bounded by the requests admitted at once (main.rs, `MAX_IN_FLIGHT`).
const CHECKS_AT_ONCE: usize = 4;
const CHECK_WAIT: Duration = Duration::from_secs(30);

thread_local! {
    /// How many of the accounts' locks this thread holds (`hold`). A password is never hashed or
    /// checked while it holds one (`checking` says so in every test that runs one): a check takes
    /// a while, longer when the server is busy, and every request needs these locks to say who it is.
    static HELD: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// One of the accounts' locks, held: counted in `HELD` while it is.
struct Held<'a, T>(std::sync::MutexGuard<'a, T>);

fn hold<T>(lock: &Mutex<T>) -> Held<'_, T> {
    let guard = lock.lock().unwrap();
    HELD.with(|h| h.set(h.get() + 1));
    Held(guard)
}

impl<T> Drop for Held<'_, T> {
    fn drop(&mut self) {
        HELD.with(|h| h.set(h.get() - 1));
    }
}

impl<T> std::ops::Deref for Held<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Held<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

/// Slots for work done a few at once: how many are taken, and how long one waits for a free one.
struct Slots {
    taken: Mutex<usize>,
    freed: std::sync::Condvar,
    most: usize,
    wait: Duration,
}

impl Slots {
    const fn new(most: usize, wait: Duration) -> Slots {
        Slots { taken: Mutex::new(0), freed: std::sync::Condvar::new(), most, wait }
    }

    /// Run `f` once a slot is free, or say the server is busy past `wait`.
    fn run<T>(&self, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        struct Slot<'a>(&'a Slots);
        impl Drop for Slot<'_> {
            fn drop(&mut self) {
                *self.0.taken.lock().unwrap() -= 1;
                self.0.freed.notify_one();
            }
        }
        let deadline = Instant::now() + self.wait;
        let mut taken = self.taken.lock().unwrap();
        while *taken >= self.most {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(BUSY.into());
            }
            taken = self.freed.wait_timeout(taken, left).unwrap().0;
        }
        *taken += 1;
        drop(taken);
        let _slot = Slot(self);
        f()
    }
}

/// Run `f` (a password hashed or checked) once one of `CHECKS_AT_ONCE` slots is free, on one of
/// as many threads kept for it (`on_a_checker`).
fn checking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    debug_assert_eq!(HELD.with(std::cell::Cell::get), 0, "a password hashed or checked holding an accounts lock: every request would wait on it");
    static CHECKS: Slots = Slots::new(CHECKS_AT_ONCE, CHECK_WAIT);
    CHECKS.run(|| on_a_checker(f))
}

/// A job for a checker thread.
type Check = Box<dyn FnOnce() + Send>;

/// Run `f` on one of `CHECKS_AT_ONCE` threads kept for checking passwords, and wait for it. Each
/// check takes argon2's 19 MiB, and the allocator keeps what a thread freed for that thread
/// (glibc's arenas): run on each request's own thread, a flood of sign-ins left the server
/// holding about 900 MB after (measured, 200 at once on 4 cores); on threads of its own, only
/// theirs hold it. A slot is had first (`checking`), so a checker is always free.
fn on_a_checker<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    static QUEUE: OnceLock<Mutex<std::sync::mpsc::Sender<Check>>> = OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let (send, receive) = std::sync::mpsc::channel::<Check>();
        let receive = std::sync::Arc::new(Mutex::new(receive));
        for i in 0..CHECKS_AT_ONCE {
            let receive = std::sync::Arc::clone(&receive);
            let spawned = std::thread::Builder::new().name(format!("holon-password-{i}")).spawn(move || {
                loop {
                    let job = receive.lock().unwrap().recv();
                    match job {
                        Ok(job) => job(),
                        Err(_) => return,
                    }
                }
            });
            if let Err(e) = spawned {
                eprintln!("holon: a password checker couldn't start: {e}");
            }
        }
        Mutex::new(send)
    });
    let (done, result) = std::sync::mpsc::channel();
    queue.lock().unwrap().send(Box::new(move || {
        let _ = done.send(f());
    })).map_err(|_| "the server's password checkers stopped".to_string())?;
    result.recv().map_err(|_| "a password check didn't finish".to_string())?
}

/// Why a password wasn't checked: every slot was taken (`checking`).
const BUSY: &str = "the server is busy checking other sign-ins: try again in a moment";

/// `password` hashed, unhurried: what `hash_password` does in a slot, and the stand-in is made
/// with (once, never refused).
fn hash_raw(password: &str) -> Result<String, String> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| e.to_string())?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| e.to_string())?;
    Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
}

fn hash_password(password: &str) -> Result<String, String> {
    let password = zeroize::Zeroizing::new(password.to_string());
    checking(move || hash_raw(&password))
}

/// Whether `password` is the one `hash` was made from; an error when no slot came free in time.
fn password_matches(password: &str, hash: &str) -> Result<bool, String> {
    let (password, hash) = (zeroize::Zeroizing::new(password.to_string()), hash.to_string());
    checking(move || Ok(PasswordHash::new(&hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())))
}

/// A hash no password is checked against to sign in, made as an account's is: checking against
/// it takes as long as checking against theirs.
fn stand_in_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| hash_raw("no account has this password").unwrap_or_default())
}

/// A setup code a person can type: 8 letters and digits that can't be misread, as `XXXX-XXXX`.
fn new_setup_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("the OS random source");
    let code: String = bytes.iter().map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char).collect();
    format!("{}-{}", &code[..4], &code[4..])
}

impl Accounts {
    /// The accounts at `path` (none yet if it doesn't exist).
    pub(crate) fn open(path: &str) -> Result<Accounts, String> {
        let mut book: Book = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("the accounts file {path:?}: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Book::default(),
            Err(e) => return Err(format!("the accounts file {path:?}: {e}")),
        };
        // An account with role agent, from before an agent was a principal of its own, is not read.
        let unread: Vec<String> = book.users.iter().filter(|p| !p.kind.may_hold(p.role)).map(|p| p.name.clone()).collect();
        if !unread.is_empty() {
            eprintln!("holon: the accounts file's {} had role agent as a person, and is not read: an agent is a principal of its own now (holon people add NAME --agent)", unread.join(", "));
            book.users.retain(|p| p.kind.may_hold(p.role));
        }
        let accounts = Accounts::with(Some(PathBuf::from(path)), book);
        if let Some(settings) = accounts.oidc_settings() {
            match crate::oidc::Oidc::discover(&settings.issuer, &settings.client_id) {
                Ok(oidc) => *accounts.oidc.write().unwrap() = Some(oidc),
                Err(e) => eprintln!("holon: signing in through {} is off until it can be read: {e}", settings.issuer),
            }
        }
        Ok(accounts)
    }

    /// Accounts kept in memory only (the tests).
    #[cfg(test)]
    pub(crate) fn memory() -> Accounts {
        Accounts::with(None, Book::default())
    }

    fn with(path: Option<PathBuf>, book: Book) -> Accounts {
        let code = no_person(&book).then(|| std::env::var("HOLON_SETUP_CODE").ok().filter(|c| !c.trim().is_empty()).unwrap_or_else(new_setup_code));
        Accounts { path, book: Mutex::new(book), setup_code: Mutex::new(code), failures: Mutex::new(HashMap::new()), setup_tries: Mutex::new(Tries::default()), oidc: RwLock::new(None), local: Mutex::new(None) }
    }

    fn save(&self, book: &Book) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let text = serde_json::to_vec_pretty(book).map_err(|e| e.to_string())?;
        crate::sealing::write_private(Path::new(path), &text).map_err(|e| format!("the accounts file {path:?}: {e}"))
    }

    /// Change the book, and keep it; expired sessions and invites go at the same time.
    fn change<T>(&self, f: impl FnOnce(&mut Book) -> Result<T, String>) -> Result<T, String> {
        let mut book = hold(&self.book);
        let now = epoch_now();
        book.sessions.retain(|s| s.expires_at > now);
        book.invites.retain(|i| i.expires_at > now);
        let out = f(&mut book)?;
        self.save(&book)?;
        Ok(out)
    }

    /// The setup code, while the first admin hasn't been created.
    pub(crate) fn setup_code(&self) -> Option<String> {
        hold(&self.setup_code).clone()
    }

    /// Whether the first admin is still to be set up: there is no person yet, whatever agents or
    /// services the run file's admin has made.
    pub(crate) fn needs_setup(&self) -> bool {
        no_person(&hold(&self.book))
    }

    fn start_session(book: &mut Book, user: &str) -> String {
        let token = new_token("hs_");
        book.sessions.push(Session { hash: hash_of(&token), user: user.into(), expires_at: epoch_now() + SESSION_SECONDS });
        token
    }

    /// The first admin, with the setup code; signed in.
    pub(crate) fn setup(&self, code: &str, name: &str, password: &str) -> Result<(String, Principal), String> {
        let expected = self.setup_code().ok_or("this server is set up already")?;
        {
            let mut tries = hold(&self.setup_tries);
            if tries.locked.is_some_and(|at| at.elapsed() < LOCKED_FOR) {
                return Err("too many wrong setup codes: wait a minute, then try again".into());
            }
            if !same(&code.trim().to_uppercase(), &expected) {
                tries.wrong += 1;
                if tries.wrong >= MAX_FAILURES {
                    (tries.wrong, tries.locked) = (0, Some(Instant::now()));
                    eprintln!("holon: {MAX_FAILURES} wrong setup codes in a row: setting up is refused for a minute");
                }
                return Err("that isn't the setup code (it is printed when the server starts)".into());
            }
            *tries = Tries::default();
        }
        valid_name(name)?;
        check_password(password)?;
        let hash = hash_password(password)?;
        let out = self.change(|book| {
            if !no_person(book) {
                return Err("this server is set up already".to_string());
            }
            if book.users.iter().any(|u| u.name == name) {
                return Err(format!("there is a principal {name:?} already: pick another name"));
            }
            let account = Principal { password: Some(hash), ..Principal::new(name, Kind::Person, Role::Admin, None) };
            book.users.push(account.clone());
            Ok((Self::start_session(book, name), account))
        })?;
        *hold(&self.setup_code) = None;
        Ok(out)
    }

    /// Sign `name` in with `password`, from nowhere in particular: a test's.
    #[cfg(test)]
    pub(crate) fn sign_in(&self, name: &str, password: &str) -> Result<(String, Principal), String> {
        self.sign_in_from(name, password, "")
    }

    /// Sign `name` in with `password`, sent from `from` (the client's address): a session token,
    /// or why not (never saying which was wrong). Wrong passwords lock the name for that address
    /// only, so someone elsewhere who knows the name can't keep its owner out; behind a proxy every
    /// address is the proxy's, and the lock is the name's, as it was.
    pub(crate) fn sign_in_from(&self, name: &str, password: &str, from: &str) -> Result<(String, Principal), String> {
        let account = hold(&self.book).users.iter().find(|u| u.name == name).cloned();
        let hash = account.as_ref().filter(|a| !a.disabled).and_then(|a| a.password.as_deref());
        // A name with no account (or no password yet, or disabled) is refused as slowly as a wrong
        // password, so how long a refusal takes doesn't say which names are accounts.
        let right = self.counted((name.to_string(), from.to_string()), || Ok(password_matches(password, hash.unwrap_or(stand_in_hash()))? && hash.is_some()))?;
        if !right {
            return Err("that name and password don't match an account here".to_string());
        }
        let account = account.unwrap();
        let token = self.change(|book| Ok(Self::start_session(book, &account.name)))?;
        Ok((token, account))
    }

    /// A password checked for `key` (a name, and where it came from), counted: refused while the
    /// key is locked, or while as many are being checked for it as may still go wrong (so tries
    /// sent at once can't try more than MAX_FAILURES before the lock). A wrong one counts, and the
    /// MAX_FAILURES-th locks the key for LOCKED_FOR; a right one clears it. `check` says whether it
    /// was right, or that it couldn't be checked (the server busy), which counts as neither.
    fn counted(&self, key: (String, String), check: impl FnOnce() -> Result<bool, String>) -> Result<bool, String> {
        {
            let mut failures = hold(&self.failures);
            forget_old_tries(&mut failures);
            let tries = failures.entry(key.clone()).or_default();
            tries.last = Some(Instant::now());
            if tries.locked.is_some_and(|at| at.elapsed() < LOCKED_FOR) {
                return Err("too many wrong passwords: wait a minute, then try again".into());
            }
            if tries.wrong + tries.checking >= MAX_FAILURES {
                return Err("too many tries at once: wait a moment, then try again".into());
            }
            tries.checking += 1;
        }
        let right = check();
        let mut failures = hold(&self.failures);
        let tries = failures.entry(key.clone()).or_default();
        tries.checking -= 1;
        let right = right?;
        if !right {
            tries.wrong += 1;
            if tries.wrong >= MAX_FAILURES {
                (tries.wrong, tries.locked) = (0, Some(Instant::now()));
            }
        } else if tries.checking == 0 {
            failures.remove(&key);
        } else {
            (tries.wrong, tries.locked) = (0, None);
        }
        Ok(right)
    }

    pub(crate) fn sign_out(&self, token: &str) {
        let hash = hash_of(token);
        let _ = self.change(|book| {
            book.sessions.retain(|s| s.hash != hash);
            Ok(())
        });
    }

    /// A new local token for OS user `user`, replacing any before it: an admin's, for this run of
    /// the server only (local.rs).
    pub(crate) fn new_local(&self, user: &str) -> String {
        let token = new_token("hl_");
        *hold(&self.local) = Some((hash_of(&token), format!("local:{user}")));
        token
    }

    /// Who the local token is, if `token` is it: a person, the OS user, an admin.
    pub(crate) fn local(&self, token: &str) -> Option<Who> {
        let local = hold(&self.local);
        let (hash, name) = local.as_ref().filter(|_| token.starts_with("hl_"))?;
        same(hash, &hash_of(token)).then(|| Who { name: name.clone(), kind: Kind::Person, role: Role::Admin })
    }

    /// The OS user the local token is for (`bma`, not `local:bma`), once there is one.
    fn local_user(&self) -> Option<String> {
        hold(&self.local).as_ref().and_then(|(_, name)| name.strip_prefix("local:").map(String::from))
    }

    /// Who a bearer token is: a session's person, or an API token's agent or service; neither
    /// when disabled.
    pub(crate) fn who(&self, token: &str) -> Option<Who> {
        let hash = hash_of(token);
        let book = hold(&self.book);
        let now = epoch_now();
        let name = if token.starts_with("hs_") {
            &book.sessions.iter().find(|s| same(&s.hash, &hash) && s.expires_at > now)?.user
        } else if token.starts_with("ht_") {
            &book.credentials.iter().find(|t| same(&t.hash, &hash))?.principal
        } else {
            return None;
        };
        let p = book.users.iter().find(|p| p.name == *name && !p.disabled)?;
        Some(Who { name: p.name.clone(), kind: p.kind, role: p.role })
    }

    /// The person someone the identity provider vouches for is, by their email.
    pub(crate) fn by_email(&self, email: &str) -> Option<Who> {
        let book = hold(&self.book);
        let p = book.users.iter().find(|p| p.kind == Kind::Person && !p.disabled && p.email.as_deref() == Some(email))?;
        Some(Who { name: p.name.clone(), kind: p.kind, role: p.role })
    }

    pub(crate) fn principal(&self, name: &str) -> Option<Principal> {
        hold(&self.book).users.iter().find(|u| u.name == name).cloned()
    }

    /// Set a new password, knowing the current one. Every other session of theirs ends (one opened
    /// with the old password by someone who had it); `kept`, the session asking, stays. The current
    /// one is checked outside the book's lock, as a sign-in's is: a check takes a while, longer
    /// when the server is busy, and every request needs that lock to say who it is. And counted, by
    /// name: MAX_FAILURES wrong lock changing it for a while, so a session someone else holds can't
    /// be used to guess it.
    pub(crate) fn change_password(&self, name: &str, current: &str, new: &str, kept: Option<&str>) -> Result<(), String> {
        check_password(new)?;
        let old = hold(&self.book).users.iter().find(|u| u.name == name).ok_or("no such account")?.password.clone();
        let right = self.counted((name.to_string(), CHANGING.to_string()), || old.as_deref().map_or(Ok(false), |h| password_matches(current, h)))?;
        if !right {
            return Err("that isn't the current password".to_string());
        }
        let hash = hash_password(new)?;
        self.change(|book| {
            let account = book.users.iter_mut().find(|u| u.name == name).ok_or("no such account")?;
            // Changed while the current one was checked (a reset, another change): what was
            // checked isn't it any more.
            if account.password != old {
                return Err("the password changed meanwhile: try again".to_string());
            }
            account.password = Some(hash);
            let kept = kept.map(hash_of);
            book.sessions.retain(|s| s.user != name || kept.as_ref().is_some_and(|k| same(&s.hash, k)));
            Ok(())
        })
    }

    /// What the editor keeps for `name` (null until it keeps something).
    pub(crate) fn prefs(&self, name: &str) -> serde_json::Value {
        hold(&self.book).users.iter().find(|u| u.name == name).map(|u| u.prefs.clone()).unwrap_or_default()
    }

    pub(crate) fn set_prefs(&self, name: &str, prefs: serde_json::Value) -> Result<(), String> {
        if serde_json::to_vec(&prefs).map_or(true, |b| b.len() > MAX_PREFS_BYTES) {
            return Err(format!("preferences are kept up to {} KiB", MAX_PREFS_BYTES / 1024));
        }
        self.change(|book| {
            book.users.iter_mut().find(|u| u.name == name).ok_or("only an account has preferences")?.prefs = prefs;
            Ok(())
        })
    }

    // ── what an admin does ─────────────────────────────────────────────────────────────────

    pub(crate) fn principals(&self) -> Vec<Principal> {
        hold(&self.book).users.clone()
    }

    /// Whether an invite or reset link for `name` is waiting.
    pub(crate) fn invited(&self, name: &str) -> bool {
        let now = epoch_now();
        hold(&self.book).invites.iter().any(|i| i.name == name && i.expires_at > now)
    }

    /// Add the person `name` with `role`: a one-time link token for them to set their password.
    pub(crate) fn invite(&self, name: &str, role: Role, email: Option<String>, by: &str) -> Result<String, String> {
        valid_name(name)?;
        held(Kind::Person, role)?;
        self.change(|book| {
            if book.users.iter().any(|u| u.name == name) {
                return Err(format!("there is a principal {name:?} already"));
            }
            let email = email.map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty());
            book.users.push(Principal { email, ..Principal::new(name, Kind::Person, role, Some(by)) });
            let token = new_token("hi_");
            book.invites.push(Invite { hash: hash_of(&token), name: name.into(), expires_at: epoch_now() + INVITE_SECONDS });
            Ok(token)
        })
    }

    /// Add the agent or service `name` with `role`, owned by `owner`, a person (a person is
    /// invited instead). Its tokens are made after.
    pub(crate) fn add(&self, name: &str, kind: Kind, role: Role, owner: &str, by: &str) -> Result<Principal, String> {
        valid_name(name)?;
        if kind == Kind::Person {
            return Err("a person is invited, to set their own password".into());
        }
        held(kind, role)?;
        let local = self.local_user();
        self.change(|book| {
            if book.users.iter().any(|u| u.name == name) {
                return Err(format!("there is a principal {name:?} already"));
            }
            may_own(book, owner, local.as_deref())?;
            let p = Principal { owner: Some(owner.into()), ..Principal::new(name, kind, role, Some(by)) };
            book.users.push(p.clone());
            Ok(p)
        })
    }

    /// A new link for the person `name` to set a new password; their sessions end, and their old
    /// password no longer signs in (a reset is often because someone else has it).
    pub(crate) fn reset(&self, name: &str) -> Result<String, String> {
        self.change(|book| {
            let p = book.users.iter_mut().find(|u| u.name == name).ok_or(format!("no principal {name:?}"))?;
            if p.kind != Kind::Person {
                return Err(format!("{name} is {}: it signs in with its tokens, not a password", p.kind.said()));
            }
            p.password = None;
            book.invites.retain(|i| i.name != name);
            book.sessions.retain(|s| s.user != name);
            let token = new_token("hi_");
            book.invites.push(Invite { hash: hash_of(&token), name: name.into(), expires_at: epoch_now() + INVITE_SECONDS });
            Ok(token)
        })
    }

    /// Who an invite link is for, while it is good.
    pub(crate) fn invite_for(&self, token: &str) -> Option<String> {
        let hash = hash_of(token);
        let now = epoch_now();
        hold(&self.book).invites.iter().find(|i| same(&i.hash, &hash) && i.expires_at > now).map(|i| i.name.clone())
    }

    /// The link's holder sets their password, and is signed in; the link is used up.
    pub(crate) fn accept(&self, token: &str, password: &str) -> Result<(String, Principal), String> {
        check_password(password)?;
        let hash = hash_password(password)?;
        let invite = hash_of(token);
        self.change(|book| {
            let now = epoch_now();
            let at = book.invites.iter().position(|i| same(&i.hash, &invite) && i.expires_at > now).ok_or("this link is used, expired or wrong: ask for a new one")?;
            let name = book.invites.remove(at).name;
            let account = book.users.iter_mut().find(|u| u.name == name).ok_or("its account is gone")?;
            account.password = Some(hash);
            let account = account.clone();
            // A new password: any session opened before it ends.
            book.sessions.retain(|s| s.user != name);
            Ok((Self::start_session(book, &name), account))
        })
    }

    /// Whether, after `f`, an admin who can sign in is left, if there was one; else the change is
    /// refused.
    fn keeping_an_admin(book: &mut Book, f: impl FnOnce(&mut Book) -> Result<(), String>) -> Result<(), String> {
        let before = std::mem::take(&mut book.users);
        book.users = before.clone();
        let an_admin = |users: &[Principal]| users.iter().any(|u| u.role == Role::Admin && !u.disabled);
        // Before the first person is set up there is none to keep (the run file's admin may
        // change the agents it made).
        let had = an_admin(&before);
        f(book)?;
        if had && !an_admin(&book.users) {
            book.users = before;
            return Err("that would leave no admin: make someone else an admin first".into());
        }
        Ok(())
    }

    pub(crate) fn set_role(&self, name: &str, role: Role) -> Result<(), String> {
        self.change(|book| {
            Self::keeping_an_admin(book, |book| {
                let p = book.users.iter_mut().find(|u| u.name == name).ok_or(format!("no principal {name:?}"))?;
                held(p.kind, role)?;
                p.role = role;
                Ok(())
            })
        })
    }

    /// Give the agent or service `name` another owner, a person.
    pub(crate) fn set_owner(&self, name: &str, owner: &str) -> Result<(), String> {
        let local = self.local_user();
        self.change(|book| {
            may_own(book, owner, local.as_deref())?;
            let p = book.users.iter_mut().find(|u| u.name == name).ok_or(format!("no principal {name:?}"))?;
            if p.kind == Kind::Person {
                return Err(format!("{name} is a person: only an agent or a service has an owner"));
            }
            p.owner = Some(owner.into());
            Ok(())
        })
    }

    /// How `name` is shown; `None` (or blank) shows it by its name.
    pub(crate) fn set_display(&self, name: &str, display: Option<String>) -> Result<(), String> {
        let display = display.map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
        if display.as_ref().is_some_and(|d| d.chars().count() > 64) {
            return Err("a display name is at most 64 characters".into());
        }
        self.change(|book| {
            book.users.iter_mut().find(|u| u.name == name).ok_or(format!("no principal {name:?}"))?.display = display;
            Ok(())
        })
    }

    /// Disable `name` (their sessions end) or enable them again.
    pub(crate) fn set_disabled(&self, name: &str, disabled: bool) -> Result<(), String> {
        self.change(|book| {
            Self::keeping_an_admin(book, |book| {
                book.users.iter_mut().find(|u| u.name == name).ok_or(format!("no principal {name:?}"))?.disabled = disabled;
                Ok(())
            })?;
            if disabled {
                book.sessions.retain(|s| s.user != name);
            }
            Ok(())
        })
    }

    /// A person's email, matched when they sign in through a provider.
    pub(crate) fn set_email(&self, name: &str, email: Option<String>) -> Result<(), String> {
        let email = email.map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty());
        self.change(|book| {
            let p = book.users.iter_mut().find(|u| u.name == name).ok_or(format!("no principal {name:?}"))?;
            if p.kind != Kind::Person && email.is_some() {
                return Err(format!("{name} is {}: only a person signs in with an email", p.kind.said()));
            }
            p.email = email;
            Ok(())
        })
    }

    /// Remove `name`, with its sessions, invites and tokens; not a person who owns an agent or a
    /// service, until it has another owner.
    pub(crate) fn remove(&self, name: &str) -> Result<(), String> {
        self.change(|book| {
            let owns: Vec<&str> = book.users.iter().filter(|p| p.owner.as_deref() == Some(name)).map(|p| p.name.as_str()).collect();
            if !owns.is_empty() {
                return Err(format!("{name} owns {}: give {} another owner first", owns.join(", "), if owns.len() == 1 { "it" } else { "them" }));
            }
            Self::keeping_an_admin(book, |book| {
                let before = book.users.len();
                book.users.retain(|u| u.name != name);
                if book.users.len() == before {
                    return Err(format!("no principal {name:?}"));
                }
                Ok(())
            })?;
            book.sessions.retain(|s| s.user != name);
            book.invites.retain(|i| i.name != name);
            book.credentials.retain(|t| t.principal != name);
            Ok(())
        })
    }

    /// The tokens of `principal`, without their hashes.
    pub(crate) fn tokens(&self, principal: &str) -> Vec<ApiToken> {
        hold(&self.book).credentials.iter().filter(|t| t.principal == principal).map(|t| ApiToken { hash: String::new(), ..t.clone() }).collect()
    }

    /// A new API token for the agent or service `principal`: shown once, here.
    pub(crate) fn create_token(&self, principal: &str, label: &str, by: &str) -> Result<(String, ApiToken), String> {
        let label = label.trim();
        if label.is_empty() || label.len() > 64 {
            return Err("a token needs a label (what it is for), at most 64 characters".into());
        }
        self.change(|book| {
            let p = book.users.iter().find(|u| u.name == principal).ok_or(format!("no principal {principal:?}"))?;
            if p.kind == Kind::Person {
                return Err(format!("{principal} is a person, who signs in with a password or a provider: a token is an agent's or a service's"));
            }
            let token = new_token("ht_");
            let mut id = [0u8; 6];
            getrandom::fill(&mut id).map_err(|e| e.to_string())?;
            let entry = ApiToken { id: crate::sealing::to_hex(&id), principal: principal.into(), label: label.into(), hash: hash_of(&token), created_at: epoch_now(), created_by: by.into() };
            book.credentials.push(entry.clone());
            Ok((token, ApiToken { hash: String::new(), ..entry }))
        })
    }

    /// Revoke `principal`'s token `id`.
    pub(crate) fn revoke_token(&self, principal: &str, id: &str) -> Result<(), String> {
        self.change(|book| {
            let before = book.credentials.len();
            book.credentials.retain(|t| !(t.id == id && t.principal == principal));
            if book.credentials.len() == before {
                return Err(format!("no token {id:?}"));
            }
            Ok(())
        })
    }

    pub(crate) fn oidc_settings(&self) -> Option<OidcSettings> {
        hold(&self.book).oidc.clone()
    }

    /// Sign in through `settings`' provider from now on (read first: refused if it can't be), or
    /// stop (None).
    pub(crate) fn set_oidc(&self, settings: Option<OidcSettings>) -> Result<(), String> {
        let found = match &settings {
            Some(s) => Some(crate::oidc::Oidc::discover(&s.issuer, &s.client_id)?),
            None => None,
        };
        self.change(|book| {
            book.oidc = settings;
            Ok(())
        })?;
        *self.oidc.write().unwrap() = found;
        Ok(())
    }

    /// The identity provider, when signing in through one is set.
    pub(crate) fn oidc(&self) -> std::sync::RwLockReadGuard<'_, Option<crate::oidc::Oidc>> {
        self.oidc.read().unwrap()
    }

    /// Use `oidc` as the provider (the tests: no discovery).
    #[cfg(test)]
    pub(crate) fn use_oidc(&self, oidc: crate::oidc::Oidc) {
        *self.oidc.write().unwrap() = Some(oidc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PW: &str = "correct horse battery";

    fn set_up() -> (Accounts, String) {
        let a = Accounts::memory();
        let code = a.setup_code().unwrap();
        let (token, _) = a.setup(&code, "ada", PW).unwrap();
        (a, token)
    }

    #[test]
    fn the_first_admin_needs_the_setup_code_and_then_there_is_none() {
        let a = Accounts::memory();
        assert!(a.needs_setup());
        let code = a.setup_code().unwrap();
        assert!(a.setup("WRNG-CODE", "ada", PW).is_err());
        assert!(a.setup(&code, "ada", "short").unwrap_err().contains("at least"));
        let (token, account) = a.setup(&code.to_lowercase(), "ada", PW).unwrap();
        assert_eq!(account.role, Role::Admin);
        assert_eq!(a.who(&token), Some(Who { name: "ada".into(), kind: Kind::Person, role: Role::Admin }));
        assert!(!a.needs_setup());
        assert_eq!(a.setup_code(), None);
        assert!(a.setup(&code, "eve", PW).is_err(), "only once");
    }

    /// An agent the run file's admin makes before anyone sets the server up (`holon mcp` makes
    /// the OS user's) leaves it to be set up: by a person, under a name of their own.
    #[test]
    fn an_agent_made_first_leaves_the_server_to_be_set_up() {
        let a = Accounts::memory();
        a.new_local("bma");
        a.add("bma-agent", Kind::Agent, Role::Agent, "bma", "local:bma").unwrap();
        assert!(a.needs_setup());
        let code = a.setup_code().unwrap();
        assert!(a.setup(&code, "bma-agent", PW).unwrap_err().contains("there is a principal"));
        a.setup(&code, "bma", PW).unwrap();
        assert!(!a.needs_setup());
        assert_eq!(a.principal("bma-agent").and_then(|p| p.owner).as_deref(), Some("bma"), "owned by the person the OS user set up as");
    }

    /// Wrong passwords from one address lock the name for that address only: its owner signs in
    /// from another. Tries under ever new names are forgotten once old.
    #[test]
    fn wrong_passwords_lock_a_name_for_their_address_only_and_old_tries_are_forgotten() {
        let (a, _) = set_up();
        for _ in 0..MAX_FAILURES {
            assert!(a.sign_in_from("ada", "wrong password!", "203.0.113.9").is_err());
        }
        assert!(a.sign_in_from("ada", PW, "203.0.113.9").unwrap_err().contains("too many"), "locked for the address that tried");
        assert!(a.sign_in_from("ada", PW, "198.51.100.4").is_ok(), "its owner, elsewhere, signs in");
        let mut failures = a.failures.lock().unwrap();
        let old = Instant::now() - LOCKED_FOR - Duration::from_secs(1);
        for i in 0..=TRIES_KEPT {
            failures.insert((format!("name-{i}"), "203.0.113.9".into()), Tries { wrong: 1, last: Some(old), ..Tries::default() });
        }
        failures.insert(("fresh".into(), "203.0.113.9".into()), Tries { wrong: 1, last: Some(Instant::now()), ..Tries::default() });
        forget_old_tries(&mut failures);
        assert!(failures.contains_key(&("fresh".to_string(), "203.0.113.9".to_string())), "a recent try is kept");
        assert!(failures.contains_key(&("ada".to_string(), "203.0.113.9".to_string())), "and a lock");
        assert!(failures.len() < 10, "the old ones forgotten: {}", failures.len());
    }

    /// No more than `CHECKS_AT_ONCE` passwords are checked at once; one more waits its turn, and
    /// past `CHECK_WAIT` is told the server is busy, not that it is wrong.
    #[test]
    fn passwords_are_checked_a_few_at_once_and_the_rest_wait_or_are_told_busy() {
        // Slots of its own, so other tests' passwords aren't held up.
        let slots = std::sync::Arc::new(Slots::new(2, Duration::from_millis(300)));
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let wait = std::sync::Arc::new(Mutex::new(wait));
        let held: Vec<_> = (0..2)
            .map(|_| {
                let (slots, wait) = (std::sync::Arc::clone(&slots), std::sync::Arc::clone(&wait));
                std::thread::spawn(move || slots.run(|| Ok(wait.lock().unwrap().recv_timeout(Duration::from_secs(10)).ok())))
            })
            .collect();
        while *slots.taken.lock().unwrap() < 2 {
            std::thread::sleep(Duration::from_millis(10));
        }
        let started = Instant::now();
        assert_eq!(slots.run(|| Ok(())).unwrap_err(), BUSY);
        assert!(started.elapsed() >= Duration::from_millis(250), "it waited its turn first");
        // A slot freed while one waits: it goes on.
        let waiter = {
            let slots = std::sync::Arc::clone(&slots);
            std::thread::spawn(move || slots.run(|| Ok(7)))
        };
        std::thread::sleep(Duration::from_millis(50));
        release.send(()).unwrap();
        assert_eq!(waiter.join().unwrap(), Ok(7), "given the slot freed");
        release.send(()).unwrap();
        for h in held {
            h.join().unwrap().unwrap();
        }
        assert_eq!(*slots.taken.lock().unwrap(), 0);
        // The server's own: CHECKS_AT_ONCE of them.
        assert_eq!((CHECKS_AT_ONCE, CHECK_WAIT), (4, Duration::from_secs(30)));
    }

    /// Wrong setup codes lock setting up for a while, as wrong passwords lock a name: the code
    /// can't be guessed at speed while the server waits for its first admin.
    #[test]
    fn wrong_setup_codes_lock_setting_up_for_a_while() {
        let a = Accounts::memory();
        let code = a.setup_code().unwrap();
        for _ in 0..MAX_FAILURES {
            assert!(a.setup("WRNG-CODE", "ada", PW).unwrap_err().contains("isn't the setup code"));
        }
        assert!(a.setup(&code, "ada", PW).unwrap_err().contains("too many"), "locked, even with the right one");
        assert!(a.needs_setup());
        // A minute on, the right code sets it up.
        a.setup_tries.lock().unwrap().locked = Some(Instant::now() - LOCKED_FOR - Duration::from_secs(1));
        let (_, account) = a.setup(&code, "ada", PW).unwrap();
        assert_eq!(account.role, Role::Admin);
        // A wrong code then right one counts afresh: one wrong code isn't a lock.
        let b = Accounts::memory();
        let code = b.setup_code().unwrap();
        assert!(b.setup("WRNG-CODE", "ada", PW).is_err());
        assert!(b.setup(&code, "ada", PW).is_ok());
    }

    #[test]
    fn signing_in_and_out_and_wrong_passwords_lock_a_name() {
        let (a, first) = set_up();
        let (token, _) = a.sign_in("ada", PW).unwrap();
        assert_ne!(token, first);
        a.sign_out(&token);
        assert_eq!(a.who(&token), None);
        assert_eq!(a.who(&first).map(|w| w.name), Some("ada".into()), "other sessions stay");
        for _ in 0..MAX_FAILURES {
            assert!(a.sign_in("ada", "wrong password!").unwrap_err().contains("don't match"));
        }
        assert!(a.sign_in("ada", PW).unwrap_err().contains("too many"), "locked, even with the right one");
        assert!(a.sign_in("nobody", PW).unwrap_err().contains("don't match"), "an unknown name says the same");
    }

    /// A name with no account is refused by the same work as a wrong password: the stand-in hash
    /// is argon2 with an account's parameters.
    #[test]
    fn an_unknown_name_is_checked_like_a_wrong_password() {
        let (stand_in, real) = (PasswordHash::new(stand_in_hash()).unwrap(), hash_password(PW).unwrap());
        let real = PasswordHash::new(&real).unwrap();
        assert_eq!((stand_in.algorithm, stand_in.version, &stand_in.params), (real.algorithm, real.version, &real.params));
        let (a, _) = set_up();
        assert!(a.sign_in("nobody", "no account has this password").unwrap_err().contains("don't match"));
    }

    /// Wrong passwords sent at once are counted before they are checked: no more than
    /// MAX_FAILURES of them are tried before the name locks.
    #[test]
    fn wrong_passwords_sent_at_once_lock_a_name_as_soon() {
        let (a, _) = set_up();
        let start = std::sync::Barrier::new(16);
        let tried = std::thread::scope(|s| {
            let runs: Vec<_> = (0..16)
                .map(|_| {
                    s.spawn(|| {
                        start.wait();
                        a.sign_in("ada", "wrong password!").unwrap_err()
                    })
                })
                .collect();
            runs.into_iter().map(|r| r.join().unwrap()).filter(|e| e.contains("don't match")).count()
        });
        assert_eq!(tried, MAX_FAILURES as usize, "each refused one waited on five being checked");
        assert!(a.sign_in("ada", PW).unwrap_err().contains("too many"), "and then it is locked");
    }

    #[test]
    fn an_invite_link_lets_its_holder_set_a_password_once() {
        let (a, _) = set_up();
        let link = a.invite("bob", Role::Editor, None, "ada").unwrap();
        assert!(a.sign_in("bob", PW).is_err(), "no password until the link is used");
        assert_eq!(a.invite_for(&link).as_deref(), Some("bob"));
        let (token, account) = a.accept(&link, PW).unwrap();
        assert_eq!((account.role, a.who(&token).unwrap().name.as_str()), (Role::Editor, "bob"));
        assert!(a.accept(&link, PW).is_err(), "used up");
        assert!(a.sign_in("bob", PW).is_ok());
        // A reset: a new link, their sessions end, and the old password no longer signs in.
        let (other, _) = a.sign_in("bob", PW).unwrap();
        let again = a.reset("bob").unwrap();
        assert_eq!((a.who(&token), a.who(&other)), (None, None));
        assert!(a.sign_in("bob", PW).is_err(), "the old password is gone with the reset");
        a.accept(&again, "another good password").unwrap();
        assert!(a.sign_in("bob", "another good password").is_ok());
    }

    #[test]
    fn a_new_password_ends_every_other_session() {
        let (a, first) = set_up();
        let (stolen, _) = a.sign_in("ada", PW).unwrap();
        assert!(a.change_password("ada", "wrong password!", "a new good password", Some(&first)).is_err());
        a.change_password("ada", PW, "a new good password", Some(&first)).unwrap();
        assert_eq!(a.who(&stolen), None, "a session opened with the old password ends");
        assert_eq!(a.who(&first).map(|w| w.name), Some("ada".into()), "the one asking stays");
        assert!(a.sign_in("ada", PW).is_err());
        assert!(a.sign_in("ada", "a new good password").is_ok());
    }

    /// `guess` (right or wrong) is counted: MAX_FAILURES wrong are each told so, and then the
    /// right one is refused too.
    fn is_counted(what: &str, guess: impl Fn(bool) -> Result<(), String>) {
        for _ in 0..MAX_FAILURES {
            let refused = guess(false).unwrap_err();
            assert!(!refused.contains("too many"), "{what}: refused before {MAX_FAILURES} wrong: {refused}");
        }
        assert!(guess(true).unwrap_err().contains("too many"), "{what}: after {MAX_FAILURES} wrong, the right one is refused too");
    }

    #[test]
    fn every_way_in_that_takes_a_secret_counts_its_wrong_guesses() {
        let new = Accounts::memory();
        let code = new.setup_code().unwrap();
        is_counted("the setup code", |right| new.setup(if right { &code } else { "WRONG-CODE" }, "ada", PW).map(drop));
        let (a, first) = set_up();
        is_counted("signing in", |right| a.sign_in("ada", if right { PW } else { "wrong password!" }).map(drop));
        is_counted("the current password, to change it", |right| a.change_password("ada", if right { PW } else { "wrong password!" }, "a new good password", Some(&first)));
    }

    /// The tripwire every test that hashes or checks a password runs: none does it holding one of
    /// the accounts' locks.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "holding an accounts lock")]
    fn a_password_checked_holding_an_accounts_lock_trips() {
        let a = Accounts::memory();
        let _book = hold(&a.book);
        let _ = password_matches(PW, stand_in_hash());
    }

    #[test]
    fn an_admin_is_always_left() {
        let (a, _) = set_up();
        assert!(a.set_role("ada", Role::Viewer).unwrap_err().contains("no admin"));
        assert!(a.set_disabled("ada", true).unwrap_err().contains("no admin"));
        assert!(a.remove("ada").unwrap_err().contains("no admin"));
        a.invite("bob", Role::Admin, None, "ada").unwrap();
        // bob is another admin (enabled, even before using his link); a disabled one doesn't count.
        a.set_disabled("bob", true).unwrap();
        assert!(a.remove("ada").is_err());
        a.set_disabled("bob", false).unwrap();
        a.remove("ada").unwrap();
        assert_eq!(a.principals().iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["bob"]);
    }

    #[test]
    fn disabling_ends_sessions_and_api_tokens_are_shown_once() {
        let (a, _) = set_up();
        let link = a.invite("cy", Role::Viewer, None, "ada").unwrap();
        let (token, _) = a.accept(&link, PW).unwrap();
        a.set_disabled("cy", true).unwrap();
        assert_eq!(a.who(&token), None);
        assert!(a.sign_in("cy", PW).is_err());

        a.add("ci", Kind::Service, Role::Editor, "ada", "ada").unwrap();
        let (secret, listed) = a.create_token("ci", "CI on GitHub", "ada").unwrap();
        assert!(secret.starts_with("ht_"));
        assert_eq!(a.who(&secret), Some(Who { name: "ci".into(), kind: Kind::Service, role: Role::Editor }), "the service, by its name");
        assert!(a.tokens("ci").iter().all(|t| t.hash.is_empty()), "never listed with its hash");
        assert!(a.revoke_token("ada", &listed.id).is_err(), "only by the token's principal");
        a.revoke_token("ci", &listed.id).unwrap();
        assert_eq!(a.who(&secret), None);
        // Disabling a service stops its tokens; enabling it lets them in again.
        let (secret, _) = a.create_token("ci", "CI on GitHub", "ada").unwrap();
        a.set_disabled("ci", true).unwrap();
        assert_eq!(a.who(&secret), None);
        a.set_disabled("ci", false).unwrap();
        assert_eq!(a.who(&secret).map(|w| w.name), Some("ci".into()));
        // Removing it removes them.
        a.remove("ci").unwrap();
        assert_eq!((a.who(&secret), a.tokens("ci").len()), (None, 0));
    }

    /// Each kind holds only its roles; a name is one principal's, whatever its kind; only a person
    /// has a password, an email and a session.
    #[test]
    fn each_kind_holds_only_its_roles() {
        let (a, _) = set_up();
        assert!(a.invite("bot", Role::Agent, None, "ada").unwrap_err().contains("a person is a viewer"));
        assert!(a.add("ci", Kind::Service, Role::Admin, "ada", "ada").unwrap_err().contains("a service is a viewer or an editor"));
        assert!(a.add("claude", Kind::Agent, Role::Editor, "ada", "ada").unwrap_err().contains("an agent is an agent or a viewer"));
        assert!(a.add("bob", Kind::Person, Role::Editor, "ada", "ada").unwrap_err().contains("invited"));
        let claude = a.add("claude", Kind::Agent, Role::Agent, "ada", "ada").unwrap();
        assert_eq!((claude.kind, claude.role, claude.owner.as_deref(), claude.created_by.as_deref()), (Kind::Agent, Role::Agent, Some("ada"), Some("ada")));
        a.set_role("claude", Role::Viewer).unwrap();
        assert!(a.set_role("claude", Role::Admin).unwrap_err().contains("an agent is"));
        assert!(a.set_role("ada", Role::Agent).unwrap_err().contains("a person is"));
        assert!(a.invite("claude", Role::Viewer, None, "ada").unwrap_err().contains("already"), "names are unique across kinds");
        assert!(a.add("ada", Kind::Service, Role::Viewer, "ada", "ada").unwrap_err().contains("already"));
        assert!(a.reset("claude").unwrap_err().contains("not a password"));
        assert!(a.set_email("claude", Some("c@example.com".into())).unwrap_err().contains("only a person"));
        assert!(a.sign_in("claude", PW).is_err());
        assert!(a.create_token("ada", "mine", "ada").unwrap_err().contains("a token is an agent's or a service's"));
        assert_eq!(a.local(&a.new_local("bma")).map(|w| (w.kind, w.role)), Some((Kind::Person, Role::Admin)), "the OS user is a person");
    }

    /// An agent's or a service's owner is a person here who can sign in, or the OS user running the
    /// server; a person who owns one stays until it has another owner.
    #[test]
    fn an_owner_is_a_person() {
        let (a, _) = set_up();
        a.add("claude", Kind::Agent, Role::Agent, "ada", "ada").unwrap();
        assert!(a.add("ci", Kind::Service, Role::Editor, "claude", "ada").unwrap_err().contains("an owner is a person"));
        assert!(a.add("ci", Kind::Service, Role::Editor, "nobody", "ada").unwrap_err().contains("no person"));
        a.invite("bob", Role::Editor, None, "ada").unwrap();
        a.set_disabled("bob", true).unwrap();
        assert!(a.add("ci", Kind::Service, Role::Editor, "bob", "ada").unwrap_err().contains("disabled"));
        a.set_disabled("bob", false).unwrap();
        a.add("ci", Kind::Service, Role::Editor, "bob", "ada").unwrap();
        assert!(a.set_owner("bob", "ada").unwrap_err().contains("only an agent or a service"));
        // The OS user owns one before it has an account here (`bma`, not `local:bma`).
        assert!(a.add("bma-agent", Kind::Agent, Role::Agent, "bma", "local:bma").is_err(), "not before the server runs as bma");
        a.new_local("bma");
        a.add("bma-agent", Kind::Agent, Role::Agent, "bma", "local:bma").unwrap();
        assert!(a.remove("bob").unwrap_err().contains("bob owns ci: give it another owner first"));
        a.set_owner("ci", "ada").unwrap();
        a.remove("bob").unwrap();
        // Disabling an owner leaves what they own as it was.
        a.invite("eve", Role::Admin, None, "ada").unwrap();
        a.set_owner("claude", "eve").unwrap();
        a.set_disabled("eve", true).unwrap();
        let (token, _) = a.create_token("claude", "the agent", "ada").unwrap();
        assert_eq!(a.who(&token).map(|w| w.name), Some("claude".into()));
    }

    #[test]
    fn each_account_keeps_its_own_preferences() {
        let (a, _) = set_up();
        assert_eq!(a.prefs("ada"), serde_json::Value::Null);
        a.set_prefs("ada", serde_json::json!({ "welcome": false })).unwrap();
        assert_eq!(a.prefs("ada")["welcome"], false);
        let big = "x".repeat(MAX_PREFS_BYTES);
        assert!(a.set_prefs("ada", serde_json::json!({ "note": big })).is_err());
        assert!(a.set_prefs("token:ci", serde_json::json!({})).is_err());
    }

    #[test]
    fn kept_as_hashes_and_read_back() {
        let path = std::env::temp_dir().join(format!("holon-accounts-{}.json", std::process::id()));
        let path = path.to_str().unwrap().to_string();
        let _ = std::fs::remove_file(&path);
        let a = Accounts::open(&path).unwrap();
        let (token, _) = a.setup(&a.setup_code().unwrap(), "ada", PW).unwrap();
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(!file.contains(&token) && !file.contains(PW), "only hashes are kept");
        let again = Accounts::open(&path).unwrap();
        assert!(!again.needs_setup());
        assert_eq!(again.who(&token).map(|w| w.name), Some("ada".into()));
        let _ = std::fs::remove_file(&path);
    }

    /// A file from before principals keeps its people; its API tokens, which had a role of their
    /// own, and its accounts with role agent are not read.
    #[test]
    fn a_file_from_before_principals_keeps_its_people() {
        let path = std::env::temp_dir().join(format!("holon-accounts-before-{}.json", std::process::id()));
        let path = path.to_str().unwrap().to_string();
        let old = serde_json::json!({
            "users": [
                { "name": "ada", "role": "admin", "created_at": 1 },
                { "name": "bot", "role": "agent", "created_at": 1 }
            ],
            "tokens": [{ "id": "a1b2c3", "label": "ci", "role": "editor", "hash": "00", "created_at": 1, "created_by": "ada" }]
        });
        std::fs::write(&path, old.to_string()).unwrap();
        let a = Accounts::open(&path).unwrap();
        assert_eq!(a.principals().iter().map(|p| (p.name.as_str(), p.kind)).collect::<Vec<_>>(), [("ada", Kind::Person)]);
        assert!(!a.needs_setup());
        let _ = std::fs::remove_file(&path);
    }
}
