//! Who may use the control plane: the gate every request passes before routing (http.rs).
//!
//! Everyone is someone: a principal (accounts.rs). A person signs in with a password, through an
//! invite link, or through the identity provider an admin set (oidc.rs); an agent or a service
//! with an API token its owner or an admin made. A request carries its session or token as
//! `Authorization: Bearer …`, and its role decides what it may do:
//!
//! - `viewer`: reads only (GET): flows, deployments, logs, a secret's names and history;
//! - `editor`: everything a viewer does, and saving, deploying, running, pausing, deleting;
//! - `admin`: everything, writing secrets, the admin panel (`/admin/…`) and managing people
//!   (`/people…`) included; a person also manages the tokens of an agent or a service they own;
//! - `agent`: what a viewer does, and proposing: storing units, previewing and deploying a flow
//!   that needs no approval it hasn't had, saving a flow, calling a deployed one (a run like any
//!   other), and a restore's dry run (deploy.rs answers it nothing more). It never approves (deploy.rs refuses its `acknowledge`), never replaces a flow
//!   someone else saved (flows.rs), and never grants, starts, stops, deletes or writes a secret:
//!   those are a person's (docs/L4s/agent-surface-L4.md).
//!
//! An agent is a principal of kind agent, signing in with its own token, never a person's: what it
//! may do is its role's, and what it did is under its own name. `holon mcp` signs in as one (on a
//! server this user runs, one it makes for them: mcphosts.rs).
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

use crate::accounts::{Accounts, Kind, Who};
use crate::http::err;
use rouille::{Request, Response};

/// What an account or token may do (see the module's notes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Viewer,
    Editor,
    Admin,
    Agent,
}

/// Who a request comes from, once let through: a principal's name (or the OS user's,
/// `local:<user>`), its kind and its role; no one for what is open (signing in, the health
/// check), and on a server without accounts (the tests).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Caller {
    pub(crate) name: Option<String>,
    pub(crate) kind: Option<Kind>,
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

/// What an agent may do besides reading (see the module's notes).
fn agent_may(method: &str, url: &str) -> bool {
    let calls = || url.strip_prefix("/deployments/").and_then(|r| r.strip_suffix("/request")).is_some_and(|slug| !slug.is_empty() && !slug.contains('/'));
    // An action by its name; whether its grant lets agents run it is the deployment's to say.
    let runs = || {
        url.strip_prefix("/trigger/").and_then(|r| r.split_once('/')).is_some_and(|(slug, name)| !slug.is_empty() && !name.is_empty() && !name.contains('/'))
    };
    // A restore, which deploy.rs answers an agent only as a dry run.
    let restores = || url.strip_prefix("/deployments/").and_then(|r| r.strip_suffix("/restore")).is_some_and(|slug| !slug.is_empty() && !slug.contains('/'));
    match method {
        "GET" => true,
        "POST" => matches!(url, "/unit" | "/deploy/preview" | "/deploy" | "/flows") || calls() || runs() || restores(),
        _ => false,
    }
}

/// What anyone signed in may do to their own account, viewers too.
fn is_own(method: &str, url: &str) -> bool {
    (method == "POST" && matches!(url, "/auth/sign-out" | "/auth/password" | "/auth/prefs")) || (method == "GET" && matches!(url, "/auth/me" | "/auth/owned" | "/auth/prefs"))
}

/// Whether `url` lists, makes or revokes the tokens of a principal: a person may for an agent or
/// a service they own (auth.rs checks they own it), as an admin may for any.
fn is_tokens_of_someone(method: &str, url: &str) -> bool {
    let Some(rest) = url.strip_prefix("/people/") else { return false };
    match rest.split_once('/') {
        Some((name, "tokens")) => !name.is_empty() && matches!(method, "GET" | "POST"),
        Some((name, rest)) => !name.is_empty() && method == "DELETE" && rest.strip_prefix("tokens/").is_some_and(|id| !id.is_empty() && !id.contains('/')),
        None => false,
    }
}

/// What only an admin may do: the admin panel and managing people.
fn is_admins(url: &str) -> bool {
    url.starts_with("/admin/") || url == "/people" || url.starts_with("/people/")
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
        // With no accounts (the tests' servers), whoever asks is let in.
        let Some(accounts) = &self.accounts else { return Ok(Caller::default()) };
        if is_open(method, &url) {
            return Ok(Caller::default());
        }
        let presented = request.header("Authorization").and_then(|h| h.strip_prefix("Bearer ")).map(str::trim).unwrap_or("");
        // The OS user who runs the server, through its run file (local.rs): its admin, set up or not.
        let who = match accounts.local(presented) {
            Some(who) => who,
            None => self.signed_in(accounts, presented)?,
        };
        let allowed = match who.role {
            Role::Admin => true,
            _ if is_own(method, &url) => true,
            Role::Viewer | Role::Editor if who.kind == Kind::Person && is_tokens_of_someone(method, &url) => true,
            _ if is_admins(&url) => false,
            Role::Viewer => method == "GET",
            Role::Editor => !is_secret_write(request),
            Role::Agent => agent_may(method, &url),
        };
        if !allowed {
            let what = if is_admins(&url) || is_secret_write(request) {
                "that needs an admin"
            } else if who.role == Role::Agent {
                "an agent proposes and calls; approving, granting, starting, stopping and deleting are a person's"
            } else {
                "a viewer only reads"
            };
            return Err(err(403, format!("{} may not do that: {what}", who.name)));
        }
        Ok(Caller { name: Some(who.name), kind: Some(who.kind), role: Some(who.role) })
    }

    /// Who a session, API token or provider's sign-in is, or why it isn't let in.
    fn signed_in(&self, accounts: &Accounts, presented: &str) -> Result<Who, Response> {
        if accounts.needs_setup() {
            // No person yet; only the run file's admin could have made an agent or a service
            // since (`holon mcp` makes the OS user's agent before anyone sets the server up).
            return accounts.who(presented).filter(|w| w.kind != Kind::Person).ok_or_else(|| err(403, "this server isn't set up yet: open the editor and enter the setup code it printed when it started"));
        }
        let refuse = |message: String| Err(err(401, message).with_additional_header("WWW-Authenticate", "Bearer"));
        if presented.is_empty() {
            return refuse("sign in first".into());
        }
        if crate::oidc::looks_like_jwt(presented) {
            let provider = accounts.oidc();
            let Some(oidc) = provider.as_ref() else { return refuse("signing in through a provider isn't set on this server".into()) };
            let identity = match oidc.verify(presented) {
                Ok(i) => i,
                Err(e) => return refuse(format!("sign-in not accepted: {e}")),
            };
            match identity.email.as_deref().and_then(|e| accounts.by_email(e)) {
                Some(who) => Ok(who),
                None => match (identity.email, identity.unverified) {
                    (None, Some(email)) => Err(err(403, format!("your identity provider gave {email} but didn't say it verified it: an account here is matched by an email the provider vouches for (email_verified); ask your provider's admin"))),
                    (email, _) => {
                        let shown = email.unwrap_or(identity.subject);
                        Err(err(403, format!("signed in as {shown}, who has no account here: ask an admin to add you with that email")))
                    }
                },
            }
        } else {
            match accounts.who(presented) {
                Some(who) => Ok(who),
                None => refuse("your sign-in has ended or isn't valid: sign in again".into()),
            }
        }
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

/// Whether `request` sets, rolls back or removes a secret: any POST or DELETE under a
/// deployment's `/secrets/`, with a name after it or not (http.rs routes one without a name to
/// the secrets too, where it is refused, but only after the gate has let it through).
pub(crate) fn is_secret_write(request: &Request) -> bool {
    let url = request.url();
    let Some(rest) = url.strip_prefix("/deployments/") else { return false };
    rest.contains("/secrets/") && matches!(request.method(), "POST" | "DELETE")
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
        assert_eq!(call(&access, "GET", "/people", Some(&bob)), Err(403));
        assert_eq!(call(&access, "GET", "/admin/oidc", Some(&bob)), Err(403));
        assert_eq!(call(&access, "POST", "/deployments/a/secrets/k", Some(&ada)), Ok(Some("ada".into())));
        assert_eq!(call(&access, "GET", "/people", Some(&ada)), Ok(Some("ada".into())));
        // The variables an env secret can name: only for whoever sets secrets.
        assert_eq!(call(&access, "GET", "/admin/secrets/env", Some(&cy)), Err(403));
        assert_eq!(call(&access, "GET", "/admin/secrets/env", Some(&bob)), Err(403));
        assert_eq!(call(&access, "GET", "/admin/secrets/env", Some(&ada)), Ok(Some("ada".into())));
        assert_eq!(call(&access, "GET", "/health", None), Ok(None));
    }

    /// A service's token acts as the service, with its role; disabled or removed, it is refused.
    #[test]
    fn a_service_acts_with_its_role() {
        let (access, _) = server();
        let accounts = access.accounts().unwrap();
        accounts.add("ci", Kind::Service, Role::Editor, "ada", "ada").unwrap();
        let (ci, _) = accounts.create_token("ci", "CI on GitHub", "ada").unwrap();
        assert_eq!(call(&access, "POST", "/deploy", Some(&ci)), Ok(Some("ci".into())));
        assert_eq!(call(&access, "GET", "/people", Some(&ci)), Err(403));
        assert_eq!(call(&access, "GET", "/people/ci/tokens", Some(&ci)), Err(403), "not a person");
        accounts.set_disabled("ci", true).unwrap();
        assert_eq!(call(&access, "POST", "/deploy", Some(&ci)), Err(401));
        accounts.set_disabled("ci", false).unwrap();
        accounts.remove("ci").unwrap();
        assert_eq!(call(&access, "POST", "/deploy", Some(&ci)), Err(401));
    }

    /// Managing people is an admin's; a person reaches the tokens routes of any name, for auth.rs
    /// to answer only those of what they own.
    #[test]
    fn people_are_an_admins_and_tokens_their_owners_too() {
        let (access, [ada, bob, cy]) = server();
        for (method, url) in [("GET", "/people"), ("POST", "/people"), ("PATCH", "/people/cy"), ("DELETE", "/people/cy"), ("POST", "/people/cy/reset")] {
            assert_eq!(call(&access, method, url, Some(&bob)), Err(403), "{method} {url}");
            assert_eq!(call(&access, method, url, Some(&ada)), Ok(Some("ada".into())), "{method} {url}");
        }
        for (method, url) in [("GET", "/people/claude/tokens"), ("POST", "/people/claude/tokens"), ("DELETE", "/people/claude/tokens/a1")] {
            assert_eq!(call(&access, method, url, Some(&bob)), Ok(Some("bob".into())), "{method} {url}");
            assert_eq!(call(&access, method, url, Some(&cy)), Ok(Some("cy".into())), "a viewer may own one too: {method} {url}");
        }
        for (method, url) in [("DELETE", "/people/claude/tokens"), ("POST", "/people/claude/tokens/a1"), ("GET", "/people/claude/tokens/a1/b"), ("GET", "/people//tokens")] {
            assert_eq!(call(&access, method, url, Some(&bob)), Err(403), "{method} {url}");
        }
    }

    #[test]
    fn an_agent_proposes_and_calls_and_does_nothing_else() {
        let (access, [ada, bob, cy]) = server();
        let accounts = access.accounts().unwrap();
        accounts.add("claude", Kind::Agent, Role::Agent, "ada", "ada").unwrap();
        let (claude, _) = accounts.create_token("claude", "Claude Code", "ada").unwrap();
        for (method, url) in [("GET", "/flows"), ("POST", "/unit"), ("POST", "/deploy/preview"), ("POST", "/deploy"), ("POST", "/flows"), ("POST", "/deployments/a/request"), ("POST", "/trigger/a/reset"), ("POST", "/deployments/a/restore"), ("GET", "/auth/me")] {
            assert_eq!(call(&access, method, url, Some(&claude)), Ok(Some("claude".into())), "{method} {url}");
        }
        for (method, url) in [
            ("POST", "/deployments/a/grant"),
            ("POST", "/deployments/a/run"),
            ("POST", "/deployments/a/pause"),
            ("POST", "/deployments/a/stop"),
            ("POST", "/deployments/a/b/restore"),
            ("DELETE", "/deployments/a"),
            ("DELETE", "/flows/a"),
            ("POST", "/deployments/a/secrets/k"),
            ("POST", "/deployments/a/b/request"),
            ("POST", "/trigger/a"),
            ("POST", "/trigger/a/b/c"),
            ("POST", "/permissions/reload"),
            ("GET", "/people"),
            ("GET", "/people/claude/tokens"),
        ] {
            assert_eq!(call(&access, method, url, Some(&claude)), Err(403), "{method} {url}");
        }
        // What an agent may not, a person may: the run file's user, an admin.
        let local = accounts.new_local("bma");
        assert_eq!(call(&access, "POST", "/deployments/a/grant", Some(&local)), Ok(Some("local:bma".into())));
        assert_eq!((call(&access, "POST", "/deploy", Some(&ada)), call(&access, "POST", "/deploy", Some(&bob)), call(&access, "POST", "/deploy", Some(&cy))), (Ok(Some("ada".into())), Ok(Some("bob".into())), Err(403)));
    }

    /// `X-Holon-Role`, which once made an editor or an admin an agent, changes nothing: whoever
    /// sends it is answered as they are, on every kind of route, and with no accounts too.
    #[test]
    fn a_role_header_changes_nothing() {
        let (access, [ada, bob, cy]) = server();
        let accounts = access.accounts().unwrap();
        accounts.add("claude", Kind::Agent, Role::Agent, "ada", "ada").unwrap();
        let (claude, _) = accounts.create_token("claude", "Claude Code", "ada").unwrap();
        let local = accounts.new_local("bma");
        let said = |access: &Access, method: &str, url: &str, bearer: &str, header: Option<&str>| {
            let mut headers = vec![("Host".to_string(), "localhost:7420".to_string()), ("Authorization".to_string(), format!("Bearer {bearer}"))];
            headers.extend(header.map(|h| ("X-Holon-Role".to_string(), h.to_string())));
            access.check(&Request::fake_http(method, url, headers, vec![])).map(|c| (c.name, c.kind, c.role)).map_err(|r| r.status_code)
        };
        let routes = [("GET", "/flows"), ("POST", "/deploy"), ("POST", "/deployments/a/grant"), ("POST", "/deployments/a/runs/r/decide"), ("POST", "/deployments/a/secrets/k"), ("GET", "/people")];
        for bearer in [&ada, &bob, &cy, &claude, &local] {
            for (method, url) in routes {
                for header in ["agent", "Agent", "admin", ""] {
                    assert_eq!(said(&access, method, url, bearer, Some(header)), said(&access, method, url, bearer, None), "{method} {url}, X-Holon-Role: {header}");
                }
            }
        }
        assert_eq!(said(&access, "POST", "/deployments/a/grant", &ada, Some("agent")), Ok((Some("ada".into()), Some(Kind::Person), Some(Role::Admin))), "an admin is answered as the admin");
        let open = Access::default();
        for (method, url) in routes {
            assert_eq!(said(&open, method, url, "", Some("agent")), Ok((None, None, None)), "no accounts: {method} {url}");
        }
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
        // The provider doesn't say it verified cy's email: not cy (a person can type any email at
        // some providers), and so no one.
        let unsaid = token(serde_json::json!({ "email": "cy@example.com" }));
        assert_eq!(call(&access, "GET", "/flows", Some(&unsaid)), Err(403), "an email not said to be verified");
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
