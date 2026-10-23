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
//! Tokens are never logged or echoed; a refusal says which token is missing, not what it is.

use zeroize::Zeroizing;

use crate::http::err;
use rouille::{Request, Response};

/// The control plane's access settings.
#[derive(Default)]
pub(crate) struct Access {
    admin: Option<Zeroizing<String>>,
    secrets: Option<Zeroizing<String>>,
    /// Host names it answers as besides loopback's (`api.example.com`, `api.example.com:7420`).
    pub(crate) hosts: Vec<String>,
    /// Editor origins it answers CORS for besides localhost's (`https://holon.example.com`).
    pub(crate) origins: Vec<String>,
}

/// The shortest token accepted: a guessable one would be no gate at all.
pub(crate) const MIN_TOKEN_LEN: usize = 24;

impl Access {
    /// Tokens as given (blank or absent: none); each at least [`MIN_TOKEN_LEN`] characters.
    pub(crate) fn new(admin: Option<String>, secrets: Option<String>, hosts: Vec<String>, origins: Vec<String>) -> Result<Access, String> {
        let token = |t: Option<String>, what: &str| -> Result<Option<Zeroizing<String>>, String> {
            match t.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
                None => Ok(None),
                Some(t) if t.chars().count() < MIN_TOKEN_LEN => Err(format!("the {what} token must be at least {MIN_TOKEN_LEN} characters")),
                Some(t) => Ok(Some(Zeroizing::new(t))),
            }
        };
        Ok(Access { admin: token(admin, "admin")?, secrets: token(secrets, "secrets")?, hosts, origins })
    }

    pub(crate) fn has_admin_token(&self) -> bool {
        self.admin.is_some()
    }

    /// Why `request` may not go on, if it may not. `secret_write`: it sets, rolls back or
    /// removes a secret.
    pub(crate) fn check(&self, request: &Request, secret_write: bool) -> Option<Response> {
        if request.method() == "OPTIONS" || (request.method() == "GET" && request.url() == "/health") {
            return None;
        }
        let presented = request
            .header("Authorization")
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(str::trim)
            .unwrap_or("");
        let matches = |t: &Option<Zeroizing<String>>| t.as_ref().is_some_and(|t| same(presented, t));
        let (needed, which) = match (secret_write, &self.secrets, &self.admin) {
            (true, Some(_), _) => (&self.secrets, "the secrets token (writing secrets is its own permission)"),
            (_, _, Some(_)) => (&self.admin, "the admin token"),
            _ => return None,
        };
        if matches(needed) {
            return None;
        }
        let refusal = if presented.is_empty() { format!("this needs {which}: send Authorization: Bearer <token>") } else { format!("not {which}") };
        Some(err(401, refusal).with_additional_header("WWW-Authenticate", "Bearer"))
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
        access.check(&request, is_secret_write(&request)).map(|r| r.status_code)
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
