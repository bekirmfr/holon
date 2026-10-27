//! The people who use this server, and how they get in: accounts, kept in `--accounts`
//! (default `.holon-accounts.json` beside the other state, owner-only).
//!
//! - **First run.** A server without accounts answers nothing but its setup: it prints a one-time
//!   setup code at startup (or takes `HOLON_SETUP_CODE`), and whoever enters it in the editor
//!   creates the first admin. Nobody else on the network can claim it first.
//! - **Accounts** have a name, a role (`viewer`: reads; `editor`: also saves, deploys, runs;
//!   `admin`: also writes secrets and manages the server) and a password, kept as an argon2id
//!   hash. Signing in gives a session token (`hs_…`, for 7 days); a few wrong passwords in a row
//!   lock that name for a minute.
//! - **Invites.** An admin adds someone by name and role and gets a one-time link (`hi_…`, for
//!   7 days): its holder sets their own password and is signed in. A reset is a new link.
//! - **API tokens** (`ht_…`), for scripts and CI: made by an admin with a role, shown once,
//!   revoked any time.
//! - **Signing in through an identity provider** (oidc.rs), optional: an admin sets the issuer
//!   and client id; someone signing in is matched to the account with their email.
//!
//! Only hashes of tokens are kept, never the tokens; passwords only as argon2id hashes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
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

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub(crate) struct Account {
    pub(crate) name: String,
    pub(crate) role: Role,
    /// Its argon2id hash; absent until an invite is accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    /// Who it is at the identity provider, when signing in through one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) email: Option<String>,
    #[serde(default)]
    pub(crate) disabled: bool,
    pub(crate) created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) created_by: Option<String>,
    /// What the editor keeps for them (its home tab's panes, whether the welcome shows on
    /// start): opaque here, so it follows them to any browser.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    prefs: serde_json::Value,
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

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub(crate) struct ApiToken {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) role: Role,
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
    #[serde(default)]
    users: Vec<Account>,
    #[serde(default)]
    invites: Vec<Invite>,
    #[serde(default)]
    sessions: Vec<Session>,
    #[serde(default)]
    tokens: Vec<ApiToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oidc: Option<OidcSettings>,
}

/// Who a token is: a name (an account, or `token:<label>`) and a role.
#[derive(Debug, PartialEq, Clone)]
pub(crate) struct Who {
    pub(crate) name: String,
    pub(crate) role: Role,
}

/// A name's sign-ins: wrong passwords since its last lock, when it was locked, and how many are
/// being checked now (each counts as wrong until it is known not to be).
#[derive(Default)]
struct Tries {
    wrong: u32,
    locked: Option<Instant>,
    checking: u32,
}

pub(crate) struct Accounts {
    path: Option<PathBuf>,
    book: Mutex<Book>,
    /// The one-time code that creates the first admin, while there is none.
    setup_code: Mutex<Option<String>>,
    failures: Mutex<HashMap<String, Tries>>,
    /// The identity provider, discovered from `book.oidc` (oidc.rs).
    oidc: RwLock<Option<crate::oidc::Oidc>>,
    /// The local token's hash and who it is (local.rs): made when the server starts, good as that
    /// OS user's admin until it stops, never stored.
    local: Mutex<Option<(String, String)>>,
}

/// A new secret token: `prefix` and 32 random bytes, base64url.
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
    if !ok || name.starts_with("token:") || name == "admin-token" {
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

fn hash_password(password: &str) -> Result<String, String> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| e.to_string())?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| e.to_string())?;
    Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
}

fn password_matches(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
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
        let book: Book = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("the accounts file {path:?}: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Book::default(),
            Err(e) => return Err(format!("the accounts file {path:?}: {e}")),
        };
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
        let code = book.users.is_empty().then(|| std::env::var("HOLON_SETUP_CODE").ok().filter(|c| !c.trim().is_empty()).unwrap_or_else(new_setup_code));
        Accounts { path, book: Mutex::new(book), setup_code: Mutex::new(code), failures: Mutex::new(HashMap::new()), oidc: RwLock::new(None), local: Mutex::new(None) }
    }

    fn save(&self, book: &Book) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let text = serde_json::to_vec_pretty(book).map_err(|e| e.to_string())?;
        crate::sealing::write_private(Path::new(path), &text).map_err(|e| format!("the accounts file {path:?}: {e}"))
    }

    /// Change the book, and keep it; expired sessions and invites go at the same time.
    fn change<T>(&self, f: impl FnOnce(&mut Book) -> Result<T, String>) -> Result<T, String> {
        let mut book = self.book.lock().unwrap();
        let now = epoch_now();
        book.sessions.retain(|s| s.expires_at > now);
        book.invites.retain(|i| i.expires_at > now);
        let out = f(&mut book)?;
        self.save(&book)?;
        Ok(out)
    }

    /// The setup code, while the first admin hasn't been created.
    pub(crate) fn setup_code(&self) -> Option<String> {
        self.setup_code.lock().unwrap().clone()
    }

    pub(crate) fn needs_setup(&self) -> bool {
        self.book.lock().unwrap().users.is_empty()
    }

    fn start_session(book: &mut Book, user: &str) -> String {
        let token = new_token("hs_");
        book.sessions.push(Session { hash: hash_of(&token), user: user.into(), expires_at: epoch_now() + SESSION_SECONDS });
        token
    }

    /// The first admin, with the setup code; signed in.
    pub(crate) fn setup(&self, code: &str, name: &str, password: &str) -> Result<(String, Account), String> {
        let expected = self.setup_code().ok_or("this server is set up already")?;
        if !same(&code.trim().to_uppercase(), &expected) {
            return Err("that isn't the setup code (it is printed when the server starts)".into());
        }
        valid_name(name)?;
        check_password(password)?;
        let hash = hash_password(password)?;
        let out = self.change(|book| {
            if !book.users.is_empty() {
                return Err("this server is set up already".to_string());
            }
            let account = Account { name: name.into(), role: Role::Admin, password: Some(hash), email: None, disabled: false, created_at: epoch_now(), created_by: None, prefs: serde_json::Value::Null };
            book.users.push(account.clone());
            Ok((Self::start_session(book, name), account))
        })?;
        *self.setup_code.lock().unwrap() = None;
        Ok(out)
    }

    /// Sign `name` in with `password`: a session token, or why not (never saying which was wrong).
    pub(crate) fn sign_in(&self, name: &str, password: &str) -> Result<(String, Account), String> {
        let refused = || "that name and password don't match an account here".to_string();
        {
            let mut failures = self.failures.lock().unwrap();
            let tries = failures.entry(name.to_string()).or_default();
            if tries.locked.is_some_and(|at| at.elapsed() < LOCKED_FOR) {
                return Err("too many wrong passwords: wait a minute, then try again".into());
            }
            // The ones still being checked count too, so sign-ins sent at once can't try more
            // than MAX_FAILURES passwords before the lock.
            if tries.wrong + tries.checking >= MAX_FAILURES {
                return Err("too many sign-ins at once: wait a moment, then try again".into());
            }
            tries.checking += 1;
        }
        let account = self.book.lock().unwrap().users.iter().find(|u| u.name == name).cloned();
        let ok = account.as_ref().is_some_and(|a| !a.disabled && a.password.as_deref().is_some_and(|h| password_matches(password, h)));
        let mut failures = self.failures.lock().unwrap();
        let tries = failures.entry(name.to_string()).or_default();
        tries.checking -= 1;
        if !ok {
            tries.wrong += 1;
            if tries.wrong >= MAX_FAILURES {
                (tries.wrong, tries.locked) = (0, Some(Instant::now()));
            }
            return Err(refused());
        }
        if tries.checking == 0 {
            failures.remove(name);
        } else {
            (tries.wrong, tries.locked) = (0, None);
        }
        drop(failures);
        let account = account.unwrap();
        let token = self.change(|book| Ok(Self::start_session(book, &account.name)))?;
        Ok((token, account))
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
        *self.local.lock().unwrap() = Some((hash_of(&token), format!("local:{user}")));
        token
    }

    /// Who the local token is, if `token` is it.
    pub(crate) fn local(&self, token: &str) -> Option<Who> {
        let local = self.local.lock().unwrap();
        let (hash, name) = local.as_ref().filter(|_| token.starts_with("hl_"))?;
        same(hash, &hash_of(token)).then(|| Who { name: name.clone(), role: Role::Admin })
    }

    /// Who a bearer token is: a session's account (not disabled), or an API token.
    pub(crate) fn who(&self, token: &str) -> Option<Who> {
        let hash = hash_of(token);
        let book = self.book.lock().unwrap();
        let now = epoch_now();
        if token.starts_with("hs_") {
            let session = book.sessions.iter().find(|s| same(&s.hash, &hash) && s.expires_at > now)?;
            let account = book.users.iter().find(|u| u.name == session.user && !u.disabled)?;
            return Some(Who { name: account.name.clone(), role: account.role });
        }
        if token.starts_with("ht_") {
            let t = book.tokens.iter().find(|t| same(&t.hash, &hash))?;
            return Some(Who { name: format!("token:{}", t.label), role: t.role });
        }
        None
    }

    /// The account someone the identity provider vouches for is, by their email.
    pub(crate) fn by_email(&self, email: &str) -> Option<Who> {
        let book = self.book.lock().unwrap();
        let account = book.users.iter().find(|u| !u.disabled && u.email.as_deref() == Some(email))?;
        Some(Who { name: account.name.clone(), role: account.role })
    }

    pub(crate) fn account(&self, name: &str) -> Option<Account> {
        self.book.lock().unwrap().users.iter().find(|u| u.name == name).cloned()
    }

    /// Set a new password, knowing the current one. Every other session of theirs ends (one opened
    /// with the old password by someone who had it); `kept`, the session asking, stays.
    pub(crate) fn change_password(&self, name: &str, current: &str, new: &str, kept: Option<&str>) -> Result<(), String> {
        check_password(new)?;
        let hash = hash_password(new)?;
        self.change(|book| {
            let account = book.users.iter_mut().find(|u| u.name == name).ok_or("no such account")?;
            if !account.password.as_deref().is_some_and(|h| password_matches(current, h)) {
                return Err("that isn't the current password".to_string());
            }
            account.password = Some(hash);
            let kept = kept.map(hash_of);
            book.sessions.retain(|s| s.user != name || kept.as_ref().is_some_and(|k| same(&s.hash, k)));
            Ok(())
        })
    }

    /// What the editor keeps for `name` (null until it keeps something).
    pub(crate) fn prefs(&self, name: &str) -> serde_json::Value {
        self.book.lock().unwrap().users.iter().find(|u| u.name == name).map(|u| u.prefs.clone()).unwrap_or_default()
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

    pub(crate) fn users(&self) -> Vec<Account> {
        self.book.lock().unwrap().users.clone()
    }

    /// Whether an invite or reset link for `name` is waiting.
    pub(crate) fn invited(&self, name: &str) -> bool {
        let now = epoch_now();
        self.book.lock().unwrap().invites.iter().any(|i| i.name == name && i.expires_at > now)
    }

    /// Add `name` with `role`: a one-time link token for them to set their password.
    pub(crate) fn invite(&self, name: &str, role: Role, email: Option<String>, by: &str) -> Result<String, String> {
        valid_name(name)?;
        self.change(|book| {
            if book.users.iter().any(|u| u.name == name) {
                return Err(format!("there is an account {name:?} already"));
            }
            let email = email.map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty());
            book.users.push(Account { name: name.into(), role, password: None, email, disabled: false, created_at: epoch_now(), created_by: Some(by.into()), prefs: serde_json::Value::Null });
            let token = new_token("hi_");
            book.invites.push(Invite { hash: hash_of(&token), name: name.into(), expires_at: epoch_now() + INVITE_SECONDS });
            Ok(token)
        })
    }

    /// A new link for `name` to set a new password; their sessions end, and their old password no
    /// longer signs in (a reset is often because someone else has it).
    pub(crate) fn reset(&self, name: &str) -> Result<String, String> {
        self.change(|book| {
            book.users.iter_mut().find(|u| u.name == name).ok_or(format!("no account {name:?}"))?.password = None;
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
        self.book.lock().unwrap().invites.iter().find(|i| same(&i.hash, &hash) && i.expires_at > now).map(|i| i.name.clone())
    }

    /// The link's holder sets their password, and is signed in; the link is used up.
    pub(crate) fn accept(&self, token: &str, password: &str) -> Result<(String, Account), String> {
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

    /// Whether, after `f`, an admin who can sign in is left; else the change is refused.
    fn keeping_an_admin(book: &mut Book, f: impl FnOnce(&mut Book) -> Result<(), String>) -> Result<(), String> {
        let before = std::mem::take(&mut book.users);
        book.users = before.clone();
        f(book)?;
        if !book.users.iter().any(|u| u.role == Role::Admin && !u.disabled) {
            book.users = before;
            return Err("that would leave no admin: make someone else an admin first".into());
        }
        Ok(())
    }

    pub(crate) fn set_role(&self, name: &str, role: Role) -> Result<(), String> {
        self.change(|book| {
            Self::keeping_an_admin(book, |book| {
                book.users.iter_mut().find(|u| u.name == name).ok_or(format!("no account {name:?}"))?.role = role;
                Ok(())
            })
        })
    }

    /// Disable `name` (their sessions end) or enable them again.
    pub(crate) fn set_disabled(&self, name: &str, disabled: bool) -> Result<(), String> {
        self.change(|book| {
            Self::keeping_an_admin(book, |book| {
                book.users.iter_mut().find(|u| u.name == name).ok_or(format!("no account {name:?}"))?.disabled = disabled;
                Ok(())
            })?;
            if disabled {
                book.sessions.retain(|s| s.user != name);
            }
            Ok(())
        })
    }

    pub(crate) fn set_email(&self, name: &str, email: Option<String>) -> Result<(), String> {
        self.change(|book| {
            book.users.iter_mut().find(|u| u.name == name).ok_or(format!("no account {name:?}"))?.email = email.map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty());
            Ok(())
        })
    }

    pub(crate) fn remove(&self, name: &str) -> Result<(), String> {
        self.change(|book| {
            Self::keeping_an_admin(book, |book| {
                let before = book.users.len();
                book.users.retain(|u| u.name != name);
                if book.users.len() == before {
                    return Err(format!("no account {name:?}"));
                }
                Ok(())
            })?;
            book.sessions.retain(|s| s.user != name);
            book.invites.retain(|i| i.name != name);
            Ok(())
        })
    }

    pub(crate) fn tokens(&self) -> Vec<ApiToken> {
        self.book.lock().unwrap().tokens.iter().map(|t| ApiToken { hash: String::new(), ..t.clone() }).collect()
    }

    /// A new API token for scripts and CI: shown once, here.
    pub(crate) fn create_token(&self, label: &str, role: Role, by: &str) -> Result<(String, ApiToken), String> {
        let label = label.trim();
        if label.is_empty() || label.len() > 64 {
            return Err("a token needs a label (what it is for), at most 64 characters".into());
        }
        self.change(|book| {
            let token = new_token("ht_");
            let mut id = [0u8; 6];
            getrandom::fill(&mut id).map_err(|e| e.to_string())?;
            let entry = ApiToken { id: crate::sealing::to_hex(&id), label: label.into(), role, hash: hash_of(&token), created_at: epoch_now(), created_by: by.into() };
            book.tokens.push(entry.clone());
            Ok((token, ApiToken { hash: String::new(), ..entry }))
        })
    }

    pub(crate) fn revoke_token(&self, id: &str) -> Result<(), String> {
        self.change(|book| {
            let before = book.tokens.len();
            book.tokens.retain(|t| t.id != id);
            if book.tokens.len() == before {
                return Err(format!("no token {id:?}"));
            }
            Ok(())
        })
    }

    pub(crate) fn oidc_settings(&self) -> Option<OidcSettings> {
        self.book.lock().unwrap().oidc.clone()
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
        assert_eq!(a.who(&token), Some(Who { name: "ada".into(), role: Role::Admin }));
        assert!(!a.needs_setup());
        assert_eq!(a.setup_code(), None);
        assert!(a.setup(&code, "eve", PW).is_err(), "only once");
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
        assert_eq!(a.users().iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["bob"]);
    }

    #[test]
    fn disabling_ends_sessions_and_api_tokens_are_shown_once() {
        let (a, _) = set_up();
        let link = a.invite("cy", Role::Viewer, None, "ada").unwrap();
        let (token, _) = a.accept(&link, PW).unwrap();
        a.set_disabled("cy", true).unwrap();
        assert_eq!(a.who(&token), None);
        assert!(a.sign_in("cy", PW).is_err());

        let (secret, listed) = a.create_token("ci", Role::Editor, "ada").unwrap();
        assert!(secret.starts_with("ht_"));
        assert_eq!(a.who(&secret), Some(Who { name: "token:ci".into(), role: Role::Editor }));
        assert!(a.tokens().iter().all(|t| t.hash.is_empty()), "never listed with its hash");
        a.revoke_token(&listed.id).unwrap();
        assert_eq!(a.who(&secret), None);
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
}
