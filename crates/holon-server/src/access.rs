//! Who may use the control plane: the gate every request passes before routing (http.rs).
//!
//! - **Loopback by default, open there.** With no token configured the server only listens on,
//!   and only answers as, this machine (Host and Origin checks, http.rs), as before: a local
//!   tool for one person.
//! - **An admin token** (`HOLON_ADMIN_TOKEN`, or `--admin-token-file PATH`): every request then
//!   needs `Authorization: Bearer <token>` (compared in constant time), except a CORS preflight
//!   and `GET /health`. Listening beyond loopback (`HOLON_SERVER_ADDR=0.0.0.0:…`) refuses to start
//!   without one, and answers only the hosts and editor origins named with `--allow-host` and
//!   `--allow-origin` besides loopback's.
//! - **A secrets token** (`HOLON_SECRETS_TOKEN`, or `--secrets-token-file PATH`): writing a
//!   secret (setting, rolling back, removing one) then needs that token, the admin token alone
//!   isn't enough. Without one, the admin token covers them. Reading a secret's names, history
//!   or audit trail never gives a value, so it is an admin read.
//!
//! - **Named users** (`--users-file PATH`, or `HOLON_USERS_FILE`): for a server several people
//!   share, each person gets a token of their own and a role. The file is a JSON list,
//!   `[{"name": "ada", "role": "editor", "token_blake3": "<hex>"}]`; it holds each token's
//!   BLAKE3 hash, never the token (`holon-server --hash-token` reads one on stdin and prints its
//!   hash). Roles:
//!   - `viewer`: reads only (GET): flows, deployments, logs, a secret's names and history;
//!   - `editor`: everything a viewer does, and saving, deploying, pausing, deleting;
//!   - `admin`: everything, writing secrets included, even with a secrets token set.
//!
//!   Users count as a gate like the admin token does: with either, every request needs one, and
//!   the server may listen beyond loopback. Who did what is kept where it matters most: each
//!   secrets audit entry names who asked (`by`).
//!
//! Tokens are never logged or echoed; a refusal says which token is missing, not what it is.

use serde::Deserialize;
use zeroize::Zeroizing;

use crate::http::err;
use rouille::{Request, Response};

/// What a named user may do (see the module's notes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Viewer,
    Editor,
    Admin,
}

/// A person with a token of their own: kept as the token's hash.
#[derive(Deserialize)]
pub(crate) struct User {
    pub(crate) name: String,
    pub(crate) role: Role,
    token_blake3: String,
}

/// Who a request comes from, once let through: a user's name, `admin` or `secrets` (a shared
/// token), or no one (an open loopback server, a preflight, the health check).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Caller {
    pub(crate) name: Option<String>,
}

/// The control plane's access settings.
#[derive(Default)]
pub(crate) struct Access {
    admin: Option<Zeroizing<String>>,
    secrets: Option<Zeroizing<String>>,
    users: Vec<User>,
    /// Host names it answers as besides loopback's (`api.example.com`, `api.example.com:7420`).
    pub(crate) hosts: Vec<String>,
    /// Editor origins it answers CORS for besides localhost's (`https://holon.example.com`).
    pub(crate) origins: Vec<String>,
}

/// The shortest token accepted: a guessable one would be no gate at all.
pub(crate) const MIN_TOKEN_LEN: usize = 24;

impl Access {
    /// Tokens as given (blank or absent: none); each at least [`MIN_TOKEN_LEN`] characters.
    #[cfg(test)]
    pub(crate) fn new(admin: Option<String>, secrets: Option<String>, hosts: Vec<String>, origins: Vec<String>) -> Result<Access, String> {
        Access::with_users(admin, secrets, Vec::new(), hosts, origins)
    }

    /// As [`Access::new`], with named users (each name once, each hash a BLAKE3 hex digest).
    pub(crate) fn with_users(admin: Option<String>, secrets: Option<String>, users: Vec<User>, hosts: Vec<String>, origins: Vec<String>) -> Result<Access, String> {
        for (i, u) in users.iter().enumerate() {
            if u.name.trim().is_empty() || u.name == "admin" || u.name == "secrets" {
                return Err(format!("a user needs a name of their own (not {:?})", u.name));
            }
            if users[..i].iter().any(|v| v.name == u.name) {
                return Err(format!("the user {:?} is named twice", u.name));
            }
            if u.token_blake3.len() != 64 || !u.token_blake3.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("{:?}'s token_blake3 isn't a BLAKE3 hash (64 hex digits): see --hash-token", u.name));
            }
        }
        let token = |t: Option<String>, what: &str| -> Result<Option<Zeroizing<String>>, String> {
            match t.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
                None => Ok(None),
                Some(t) if t.chars().count() < MIN_TOKEN_LEN => Err(format!("the {what} token must be at least {MIN_TOKEN_LEN} characters")),
                Some(t) => Ok(Some(Zeroizing::new(t))),
            }
        };
        let users = users.into_iter().map(|u| User { token_blake3: u.token_blake3.to_ascii_lowercase(), ..u }).collect();
        Ok(Access { admin: token(admin, "admin")?, secrets: token(secrets, "secrets")?, users, hosts, origins })
    }

    /// Whether every request needs a token: an admin token or named users are set.
    pub(crate) fn is_gated(&self) -> bool {
        self.admin.is_some() || !self.users.is_empty()
    }

    /// How many named users there are.
    pub(crate) fn user_count(&self) -> usize {
        self.users.len()
    }

    /// Who `request` comes from, or why it may not go on. `secret_write`: it sets, rolls back or
    /// removes a secret.
    pub(crate) fn check(&self, request: &Request, secret_write: bool) -> Result<Caller, Response> {
        if request.method() == "OPTIONS" || (request.method() == "GET" && request.url() == "/health") {
            return Ok(Caller::default());
        }
        let presented = request
            .header("Authorization")
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(str::trim)
            .unwrap_or("");
        let matches = |t: &Option<Zeroizing<String>>| !presented.is_empty() && t.as_ref().is_some_and(|t| same(presented, t));
        let user = (!presented.is_empty() && !self.users.is_empty())
            .then(|| hash_token(presented))
            .and_then(|hash| self.users.iter().find(|u| same(&hash, &u.token_blake3)));
        let caller = |name: &str| Ok(Caller { name: Some(name.to_string()) });
        let refuse = |which: &str| {
            let refusal = if presented.is_empty() { format!("this needs {which}: send Authorization: Bearer <token>") } else { format!("not {which}") };
            Err(err(401, refusal).with_additional_header("WWW-Authenticate", "Bearer"))
        };
        if secret_write {
            if matches(&self.secrets) {
                return caller("secrets");
            }
            if let Some(u) = user {
                return if u.role == Role::Admin { caller(&u.name) } else { Err(err(403, format!("{} may not write secrets: that needs an admin", u.name))) };
            }
            if self.secrets.is_some() {
                return refuse("the secrets token, or an admin's own (writing secrets is its own permission)");
            }
        }
        if matches(&self.admin) {
            return caller("admin");
        }
        if let Some(u) = user {
            return if u.role == Role::Viewer && request.method() != "GET" { Err(err(403, format!("{} is a viewer: reads only", u.name))) } else { caller(&u.name) };
        }
        if self.is_gated() {
            return refuse(if self.users.is_empty() { "the admin token" } else { "a user's own token (or the admin token)" });
        }
        Ok(Caller::default())
    }

    /// Whether `origin` is an editor origin this server answers CORS for: localhost's, or one
    /// named with `--allow-origin`.
    pub(crate) fn allows_origin(&self, origin: &str) -> bool {
        crate::http::is_allowed_origin(origin) || self.origins.iter().any(|o| o == origin)
    }

    /// Whether `host` (a request's Host header) names this server: on loopback at `port`, or a
    /// host named with `--allow-host`.
    pub(crate) fn allows_host(&self, host: &str, port: u16) -> bool {
        let loopback = ["localhost", "127.0.0.1", "[::1]"].iter().any(|name| host == format!("{name}:{port}") || (port == 80 && host == *name));
        loopback || self.hosts.iter().any(|h| host == h || host == format!("{h}:{port}"))
    }
}

/// Whether two tokens are equal, compared in constant time (their length aside: a token's length
/// is no secret).
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether `request` sets, rolls back or removes a secret.
pub(crate) fn is_secret_write(request: &Request) -> bool {
    let url = request.url();
    let Some(rest) = url.strip_prefix("/deployments/") else { return false };
    let Some((_, after)) = rest.split_once("/secrets/") else { return false };
    !after.is_empty() && matches!(request.method(), "POST" | "DELETE")
}

/// The named users in `path`, else in the file `HOLON_USERS_FILE` names (a JSON list; see the
/// module's notes); none without either.
pub(crate) fn users_from(path: Option<&str>) -> Result<Vec<User>, String> {
    let Some(path) = path.map(String::from).or_else(|| std::env::var("HOLON_USERS_FILE").ok()).filter(|p| !p.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("could not read the users file {path:?}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("the users file {path:?}: {e} (expected a list of {{name, role: viewer|editor|admin, token_blake3}})"))
}

/// A token's hash as a users file keeps it.
pub(crate) fn hash_token(token: &str) -> String {
    blake3::hash(token.trim().as_bytes()).to_hex().to_string()
}

/// A token from the environment variable `var`, or the file `file` (its contents, trimmed).
pub(crate) fn token_from(var: &str, file: Option<&str>) -> Result<Option<String>, String> {
    if let Some(path) = file {
        return std::fs::read_to_string(path).map(|t| Some(t.trim().to_string())).map_err(|e| format!("could not read {path:?}: {e}"));
    }
    Ok(std::env::var(var).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADMIN: &str = "admin-token-0123456789abcdef";
    const SECRETS: &str = "secrets-token-0123456789abcdef";

    fn call(access: &Access, method: &str, url: &str, bearer: Option<&str>) -> Option<u16> {
        let mut headers = vec![("Host".to_string(), "localhost:7420".to_string())];
        if let Some(b) = bearer {
            headers.push(("Authorization".to_string(), format!("Bearer {b}")));
        }
        let request = Request::fake_http(method, url, headers, vec![]);
        access.check(&request, is_secret_write(&request)).err().map(|r| r.status_code)
    }

    #[test]
    fn with_no_token_it_stays_open_on_loopback() {
        let open = Access::default();
        assert_eq!(call(&open, "GET", "/deployments", None), None);
        assert_eq!(call(&open, "POST", "/deployments/a/secrets/k", None), None);
    }

    #[test]
    fn an_admin_token_gates_every_request_but_preflight_and_health() {
        let access = Access::new(Some(ADMIN.into()), None, vec![], vec![]).unwrap();
        assert_eq!(call(&access, "GET", "/deployments", None), Some(401));
        assert_eq!(call(&access, "GET", "/deployments", Some("wrong-token-0123456789abcdef")), Some(401));
        assert_eq!(call(&access, "GET", "/deployments", Some(ADMIN)), None);
        assert_eq!(call(&access, "OPTIONS", "/deployments", None), None);
        assert_eq!(call(&access, "GET", "/health", None), None);
        // No secrets token: the admin token covers writing secrets.
        assert_eq!(call(&access, "POST", "/deployments/a/secrets/k", Some(ADMIN)), None);
    }

    #[test]
    fn writing_secrets_is_its_own_permission_when_a_secrets_token_is_set() {
        let access = Access::new(Some(ADMIN.into()), Some(SECRETS.into()), vec![], vec![]).unwrap();
        for (method, url) in [("POST", "/deployments/a/secrets/k"), ("POST", "/deployments/a/secrets/k/rollback"), ("DELETE", "/deployments/a/secrets/k")] {
            assert_eq!(call(&access, method, url, Some(ADMIN)), Some(401), "{method} {url}: the admin token isn't enough");
            assert_eq!(call(&access, method, url, Some(SECRETS)), None);
        }
        // Reading names and history is an admin read; the secrets token opens nothing else.
        assert_eq!(call(&access, "GET", "/deployments/a/secrets", Some(ADMIN)), None);
        assert_eq!(call(&access, "GET", "/deployments/a/secrets/k", Some(ADMIN)), None);
        assert_eq!(call(&access, "GET", "/deployments", Some(SECRETS)), Some(401));
    }

    fn user(name: &str, role: Role, token: &str) -> User {
        User { name: name.into(), role, token_blake3: hash_token(token) }
    }

    const ADA: &str = "ada-own-token-0123456789abcdef";
    const BOB: &str = "bob-own-token-0123456789abcdef";
    const CY: &str = "cy-own-token-0123456789abcdef";

    #[test]
    fn named_users_act_by_role_and_are_named() {
        let users = vec![user("ada", Role::Admin, ADA), user("bob", Role::Editor, BOB), user("cy", Role::Viewer, CY)];
        let access = Access::with_users(None, Some(SECRETS.into()), users, vec![], vec![]).unwrap();
        assert!(access.is_gated(), "users gate the server like an admin token");
        assert_eq!(call(&access, "GET", "/deployments", None), Some(401));
        assert_eq!(call(&access, "GET", "/deployments", Some("nobody-token-0123456789abcdef")), Some(401));
        // A viewer reads; an editor changes; only an admin writes secrets.
        assert_eq!(call(&access, "GET", "/flows", Some(CY)), None);
        assert_eq!(call(&access, "POST", "/flows", Some(CY)), Some(403));
        assert_eq!(call(&access, "POST", "/flows", Some(BOB)), None);
        assert_eq!(call(&access, "POST", "/deployments/a/secrets/k", Some(BOB)), Some(403));
        assert_eq!(call(&access, "POST", "/deployments/a/secrets/k", Some(ADA)), None, "an admin's own token, though a secrets token is set");
        assert_eq!(call(&access, "POST", "/deployments/a/secrets/k", Some(SECRETS)), None);
        // And each is named, for the audit.
        let named = |token: &str| {
            let request = Request::fake_http("POST", "/deployments/a/secrets/k", vec![("Authorization".into(), format!("Bearer {token}"))], vec![]);
            access.check(&request, true).unwrap().name
        };
        assert_eq!(named(ADA).as_deref(), Some("ada"));
        assert_eq!(named(SECRETS).as_deref(), Some("secrets"));
    }

    #[test]
    fn a_users_file_needs_distinct_names_and_real_hashes() {
        assert!(Access::with_users(None, None, vec![user("ada", Role::Admin, ADA), user("ada", Role::Viewer, BOB)], vec![], vec![]).is_err());
        assert!(Access::with_users(None, None, vec![user("admin", Role::Admin, ADA)], vec![], vec![]).is_err());
        let bad = User { name: "ada".into(), role: Role::Admin, token_blake3: "not-a-hash".into() };
        assert!(Access::with_users(None, None, vec![bad], vec![], vec![]).is_err());
        let parsed: Vec<User> = serde_json::from_str(&format!(r#"[{{"name": "ada", "role": "editor", "token_blake3": "{}"}}]"#, hash_token(ADA))).unwrap();
        assert_eq!(parsed[0].role, Role::Editor);
    }

    #[test]
    fn a_short_token_and_unnamed_hosts_and_origins_are_refused() {
        assert!(Access::new(Some("short".into()), None, vec![], vec![]).is_err());
        let access = Access::new(Some(ADMIN.into()), None, vec!["api.example.com".into()], vec!["https://holon.example.com".into()]).unwrap();
        assert!(access.allows_host("api.example.com:7420", 7420) && access.allows_host("localhost:7420", 7420));
        assert!(!access.allows_host("evil.example.com:7420", 7420));
        assert!(access.allows_origin("https://holon.example.com") && access.allows_origin("http://localhost:5173"));
        assert!(!access.allows_origin("https://evil.example.com"));
    }
}
