//! Who may use the control plane: the gate every request passes before routing (http.rs).
//!
//! Everyone is someone: an account (accounts.rs), signed in with a password, through an invite
//! link, or through the identity provider an admin set (oidc.rs); or an API token an admin made
//! for a script. A request carries its session or token as `Authorization: Bearer …`, and its
//! role decides what it may do:
//!
//! - `viewer`: reads only (GET): flows, deployments, logs, a secret's names and history;
//! - `editor`: everything a viewer does, and saving, deploying, running, pausing, deleting;
//! - `admin`: everything, writing secrets and the admin panel (`/admin/…`) included.
//!
//! Anyone may change their own password and sign out. Open without signing in: a CORS
//! preflight, the health check, and signing in itself (`/auth/…`: the server's state, setup,
//! sign-in, invite links, where to sign in through a provider). A server without accounts yet
//! answers nothing else until its first admin is set up (with the code it prints).
//!
//! The OS user running the server signs in through its run file (local.rs): a token made for
//! this run, good as an admin until it stops, that only that user can read.
//!
//! Hosts and origins: the server answers as loopback, and as the hosts and editor origins named
//! with `--allow-host` / `--allow-origin`. Tokens are never logged or echoed.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::accounts::{Accounts, Who};
use crate::http::err;
use rouille::{Request, Response};

/// What an account or token may do (see the module's notes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Viewer,
    Editor,
    Admin,
}

/// Who a request comes from, once let through: an account's name or `token:<label>`, and its
/// role; no one for what is open (signing in, the health check), and on a server without
/// accounts (the tests).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Caller {
    pub(crate) name: Option<String>,
    pub(crate) role: Option<Role>,
}

/// The control plane's access settings.
#[derive(Default)]
pub(crate) struct Access {
    /// The accounts; none only in the tests, where the control plane is open.
    accounts: Option<Arc<Accounts>>,
    /// Host names it answers as besides loopback's (`api.example.com`, `api.example.com:7420`).
    pub(crate) hosts: Vec<String>,
    /// Editor origins it answers CORS for besides localhost's (`https://holon.example.com`).
    pub(crate) origins: Vec<String>,
}

/// What may be asked without signing in.
fn is_open(method: &str, url: &str) -> bool {
    method == "OPTIONS"
        || (method == "GET" && (url == "/health" || url == "/auth/state" || url == "/auth/config" || url == "/auth/invite"))
        || (method == "POST" && matches!(url, "/auth/setup" | "/auth/sign-in" | "/auth/invite"))
}

/// What anyone signed in may do to their own account, viewers too.
fn is_own(method: &str, url: &str) -> bool {
    (method == "POST" && matches!(url, "/auth/sign-out" | "/auth/password" | "/auth/prefs")) || (method == "GET" && matches!(url, "/auth/me" | "/auth/prefs"))
}

impl Access {
    pub(crate) fn new(accounts: Arc<Accounts>, hosts: Vec<String>, origins: Vec<String>) -> Access {
        Access { accounts: Some(accounts), hosts, origins }
    }

    pub(crate) fn accounts(&self) -> Option<&Arc<Accounts>> {
        self.accounts.as_ref()
    }

    /// Who `request` comes from, or why it may not go on.
    pub(crate) fn check(&self, request: &Request) -> Result<Caller, Response> {
        let (method, url) = (request.method(), request.url());
        let Some(accounts) = &self.accounts else { return Ok(Caller::default()) };
        if is_open(method, &url) {
            return Ok(Caller::default());
        }
        let presented = request.header("Authorization").and_then(|h| h.strip_prefix("Bearer ")).map(str::trim).unwrap_or("");
        // The OS user who runs the server, through its run file (local.rs): its admin, set up or not.
        if let Some(who) = accounts.local(presented) {
            return Ok(Caller { name: Some(who.name), role: Some(who.role) });
        }
        if accounts.needs_setup() {
            return Err(err(403, "this server isn't set up yet: open the editor and enter the setup code it printed when it started"));
        }
        let refuse = |message: String| Err(err(401, message).with_additional_header("WWW-Authenticate", "Bearer"));
        if presented.is_empty() {
            return refuse("sign in first".into());
        }
        let who: Who = if crate::oidc::looks_like_jwt(presented) {
            let provider = accounts.oidc();
            let Some(oidc) = provider.as_ref() else { return refuse("signing in through a provider isn't set on this server".into()) };
            let identity = match oidc.verify(presented) {
                Ok(i) => i,
                Err(e) => return refuse(format!("sign-in not accepted: {e}")),
            };
            match identity.email.as_deref().and_then(|e| accounts.by_email(e)) {
                Some(who) => who,
                None => {
                    let shown = identity.email.unwrap_or(identity.subject);
                    return Err(err(403, format!("signed in as {shown}, who has no account here: ask an admin to add you with that email")));
                }
            }
        } else {
            match accounts.who(presented) {
                Some(who) => who,
                None => return refuse("your sign-in has ended or isn't valid: sign in again".into()),
            }
        };
        let allowed = match who.role {
            Role::Admin => true,
            _ if is_own(method, &url) => true,
            _ if url.starts_with("/admin/") => false,
            Role::Viewer => method == "GET",
            Role::Editor => !is_secret_write(request),
        };
        if !allowed {
            let what = if url.starts_with("/admin/") || is_secret_write(request) { "that needs an admin" } else { "a viewer only reads" };
            return Err(err(403, format!("{} may not do that: {what}", who.name)));
        }
        Ok(Caller { name: Some(who.name), role: Some(who.role) })
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

/// Whether `request` sets, rolls back or removes a secret.
pub(crate) fn is_secret_write(request: &Request) -> bool {
    let url = request.url();
    let Some(rest) = url.strip_prefix("/deployments/") else { return false };
    let Some((_, after)) = rest.split_once("/secrets/") else { return false };
    !after.is_empty() && matches!(request.method(), "POST" | "DELETE")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PW: &str = "correct horse battery";

    /// A server with ada (admin), bob (editor) and cy (viewer), each signed in.
    fn server() -> (Access, [String; 3]) {
        let accounts = Arc::new(Accounts::memory());
        let (ada, _) = accounts.setup(&accounts.setup_code().unwrap(), "ada", PW).unwrap();
        let signed = |name: &str, role: Role| accounts.accept(&accounts.invite(name, role, None, "ada").unwrap(), PW).unwrap().0;
        let (bob, cy) = (signed("bob", Role::Editor), signed("cy", Role::Viewer));
        (Access::new(accounts, vec![], vec![]), [ada, bob, cy])
    }

    fn call(access: &Access, method: &str, url: &str, bearer: Option<&str>) -> Result<Option<String>, u16> {
        let mut headers = vec![("Host".to_string(), "localhost:7420".to_string())];
        if let Some(b) = bearer {
            headers.push(("Authorization".to_string(), format!("Bearer {b}")));
        }
        access.check(&Request::fake_http(method, url, headers, vec![])).map(|c| c.name).map_err(|r| r.status_code)
    }

    #[test]
    fn a_new_server_answers_only_its_setup() {
        let access = Access::new(Arc::new(Accounts::memory()), vec![], vec![]);
        assert_eq!(call(&access, "GET", "/flows", None), Err(403));
        assert_eq!(call(&access, "GET", "/auth/state", None), Ok(None));
        assert_eq!(call(&access, "POST", "/auth/setup", None), Ok(None));
    }

    #[test]
    fn each_role_does_what_it_may() {
        let (access, [ada, bob, cy]) = server();
        assert_eq!(call(&access, "GET", "/flows", None), Err(401));
        assert_eq!(call(&access, "GET", "/flows", Some("hs_not-a-session")), Err(401));
        assert_eq!(call(&access, "GET", "/flows", Some(&cy)), Ok(Some("cy".into())));
        assert_eq!(call(&access, "POST", "/flows", Some(&cy)), Err(403));
        assert_eq!(call(&access, "POST", "/auth/password", Some(&cy)), Ok(Some("cy".into())), "their own password");
        assert_eq!(call(&access, "POST", "/flows", Some(&bob)), Ok(Some("bob".into())));
        assert_eq!(call(&access, "POST", "/deployments/a/secrets/k", Some(&bob)), Err(403));
        assert_eq!(call(&access, "GET", "/admin/users", Some(&bob)), Err(403));
        assert_eq!(call(&access, "POST", "/deployments/a/secrets/k", Some(&ada)), Ok(Some("ada".into())));
        assert_eq!(call(&access, "GET", "/admin/users", Some(&ada)), Ok(Some("ada".into())));
        assert_eq!(call(&access, "GET", "/health", None), Ok(None));
    }

    #[test]
    fn api_tokens_act_with_their_role() {
        let (access, _) = server();
        let accounts = access.accounts().unwrap();
        let (ci, _) = accounts.create_token("ci", Role::Editor, "ada").unwrap();
        assert_eq!(call(&access, "POST", "/deploy", Some(&ci)), Ok(Some("token:ci".into())));
        assert_eq!(call(&access, "GET", "/admin/users", Some(&ci)), Err(403));
    }

    #[test]
    fn signed_in_through_a_provider_is_matched_by_email() {
        use crate::oidc::tests::{provider, token};
        let (access, _) = server();
        let accounts = access.accounts().unwrap();
        accounts.set_email("cy", Some("cy@example.com".into())).unwrap();
        accounts.use_oidc(provider());
        let cy = token(serde_json::json!({ "email": "cy@example.com", "email_verified": true }));
        assert_eq!(call(&access, "GET", "/flows", Some(&cy)), Ok(Some("cy".into())));
        assert_eq!(call(&access, "POST", "/flows", Some(&cy)), Err(403), "still a viewer");
        let eve = token(serde_json::json!({ "email": "eve@example.com", "email_verified": true }));
        assert_eq!(call(&access, "GET", "/flows", Some(&eve)), Err(403), "no account with that email");
        let expired = token(serde_json::json!({ "email": "cy@example.com", "exp": crate::deploy::epoch_now() - 3600 }));
        assert_eq!(call(&access, "GET", "/flows", Some(&expired)), Err(401));
    }

    #[test]
    fn unnamed_hosts_and_origins_are_refused() {
        let access = Access { accounts: None, hosts: vec!["api.example.com".into()], origins: vec!["https://holon.example.com".into()] };
        assert!(access.allows_host("api.example.com:7420", 7420) && access.allows_host("localhost:7420", 7420));
        assert!(!access.allows_host("evil.example.com:7420", 7420));
        assert!(access.allows_origin("https://holon.example.com") && access.allows_origin("http://localhost:5173"));
        assert!(!access.allows_origin("https://evil.example.com"));
    }
}
