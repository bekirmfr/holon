//! Who may use the control plane: the gate every request passes before routing (http.rs).
//!
//! Everyone is someone: a principal (accounts.rs). A person signs in with a password, through an
//! invite link, or through the identity provider an admin set (oidc.rs); an agent or a service
//! with an API token its owner or an admin made. A request carries its session or token as
//! `Authorization: Bearer …`.
//!
//! The gate finds the request's route in can.rs's table, which says what it asks: nothing (a CORS
//! preflight, the health check, and signing in itself, `/auth/…`: the server's state, setup,
//! sign-in, invite links, where to sign in through a provider); that its caller is signed in
//! (their own account: their password, their preferences, signing out); or that `can` lets its
//! caller do an action, as a role bound to them holds it (bindings.rs says what is bound to whom,
//! can.rs what each role holds). A path no route
//! has is answered 404, once its caller is known. A server without accounts yet answers nothing
//! but what is open until its first admin is set up (with the code it prints).
//!
//! An agent is a principal of kind agent, signing in with its own token, never a person's: what it
//! may do is what the roles bound to it hold, and what it did is under its own name. `holon mcp` signs in as one (on a
//! server this user runs, one it makes for them: mcphosts.rs).
//!
//! The OS user running the server signs in through its run file (local.rs): a token made for
//! this run, good as an admin until it stops, that only that user can read.
//!
//! Hosts and origins: the server answers as loopback, and as the hosts and editor origins named
//! with `--allow-host` / `--allow-origin`. Tokens are never logged or echoed.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::accounts::{Accounts, Kind, Who};
use crate::bindings::Held;
use crate::can::{self, Route, Rule};
use crate::http::err;
use rouille::{Request, Response};

/// A built-in role (can.rs), as a binding holds it (bindings.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Viewer,
    Editor,
    Admin,
    Agent,
}

impl Role {
    /// As the access document and its versions write it: `editor`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Editor => "editor",
            Role::Admin => "admin",
            Role::Agent => "agent",
        }
    }
}

/// Who a request comes from, once let through: a principal's name (or the OS user's,
/// `local:<user>`), its kind and what it holds (bindings.rs); no one for what is open (signing
/// in, the health check), and on a server without accounts (the tests), who holds nothing at all
/// and is let do everything (`holds` none).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Caller {
    pub(crate) name: Option<String>,
    pub(crate) kind: Option<Kind>,
    pub(crate) holds: Option<Arc<[Held]>>,
}

impl Caller {
    /// The widest built-in role it holds on the host: what the editor shows it as.
    pub(crate) fn host_role(&self) -> Option<Role> {
        crate::bindings::host_role(self.holds.as_deref().unwrap_or_default(), self.kind?)
    }

    /// `name`, of `kind`, holding `role` on the host (the tests).
    #[cfg(test)]
    pub(crate) fn holding(name: &str, kind: Kind, role: Role) -> Caller {
        let held = Held::new(Arc::clone(crate::can::role(role)), crate::bindings::Scope::Host);
        Caller { name: Some(name.into()), kind: Some(kind), holds: Some(Arc::from([held])) }
    }
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

impl Access {
    pub(crate) fn new(accounts: Arc<Accounts>, hosts: Vec<String>, origins: Vec<String>) -> Access {
        Access { accounts: Some(accounts), hosts, origins }
    }

    pub(crate) fn accounts(&self) -> Option<&Arc<Accounts>> {
        self.accounts.as_ref()
    }

    /// Who `request` comes from, once its route lets them, or why it may not go on.
    pub(crate) fn admit(&self, request: &Request) -> Result<Caller, Response> {
        let url = request.url();
        let found = can::find(request.method(), &url);
        if let Some((Route { rule: Rule::Open, .. }, _)) = found {
            return Ok(Caller::default());
        }
        let caller = self.authenticate(request)?;
        let Some((route, parts)) = found else { return Err(Response::empty_404()) };
        can::may(&caller, route, &parts, self.accounts.as_deref()).map_err(|refusal| refusal.response())?;
        Ok(caller)
    }

    /// Who `request` comes from, and what they hold, or why they aren't let in: with no accounts
    /// (the tests' servers), no one, whoever asks; the OS user who runs the server, through its
    /// run file (local.rs), admin on the host whether it is set up or not; or whoever its session
    /// or token is, holding what is bound to them.
    fn authenticate(&self, request: &Request) -> Result<Caller, Response> {
        let Some(accounts) = &self.accounts else { return Ok(Caller::default()) };
        let presented = request.header("Authorization").and_then(|h| h.strip_prefix("Bearer ")).map(str::trim).unwrap_or("");
        let (who, holds) = match accounts.local(presented) {
            Some(who) => (who, Held::admin()),
            None => {
                let who = self.signed_in(accounts, presented)?;
                let holds = accounts.holding(&who.name);
                (who, holds)
            }
        };
        Ok(Caller { name: Some(who.name), kind: Some(who.kind), holds: Some(holds) })
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

#[cfg(test)]
mod tests {
    use super::*;

    const PW: &str = "correct horse battery";

    /// A server with ada (admin), bob (editor) and cy (viewer), each signed in.
    fn server() -> (Access, [String; 3]) {
        let accounts = Arc::new(Accounts::memory());
        let (ada, _) = accounts.setup(&accounts.setup_code().unwrap(), "ada", PW).unwrap();
        let signed = |name: &str, role: Role| accounts.accept(&accounts.invite(name, role.name(), None, "ada").unwrap(), PW).unwrap().0;
        let (bob, cy) = (signed("bob", Role::Editor), signed("cy", Role::Viewer));
        (Access::new(accounts, vec![], vec![]), [ada, bob, cy])
    }

    fn call(access: &Access, method: &str, url: &str, bearer: Option<&str>) -> Result<Option<String>, u16> {
        let mut headers = vec![("Host".to_string(), "localhost:7420".to_string())];
        if let Some(b) = bearer {
            headers.push(("Authorization".to_string(), format!("Bearer {b}")));
        }
        access.admit(&Request::fake_http(method, url, headers, vec![])).map(|c| c.name).map_err(|r| r.status_code)
    }

    #[test]
    fn a_new_server_answers_only_its_setup() {
        let access = Access::new(Arc::new(Accounts::memory()), vec![], vec![]);
        assert_eq!(call(&access, "GET", "/flows", None), Err(403));
        assert_eq!(call(&access, "GET", "/auth/state", None), Ok(None));
        assert_eq!(call(&access, "POST", "/auth/setup", None), Ok(None));
    }

    /// A service's token acts as the service, with its role; disabled or removed, it is refused.
    #[test]
    fn a_service_acts_with_its_role() {
        let (access, _) = server();
        let accounts = access.accounts().unwrap();
        accounts.add("ci", Kind::Service, "editor", "ada", "ada").unwrap();
        let (ci, _) = accounts.create_token("ci", "CI on GitHub", "ada").unwrap();
        assert_eq!(call(&access, "POST", "/deploy", Some(&ci)), Ok(Some("ci".into())));
        assert_eq!(call(&access, "GET", "/people", Some(&ci)), Err(403));
        assert_eq!(call(&access, "GET", "/people/ci/tokens", Some(&ci)), Err(403), "not a person");
        accounts.set_disabled("ci", true).unwrap();
        assert_eq!(call(&access, "POST", "/deploy", Some(&ci)), Err(401));
        accounts.set_disabled("ci", false).unwrap();
        accounts.remove("ci", "ada").unwrap();
        assert_eq!(call(&access, "POST", "/deploy", Some(&ci)), Err(401));
    }

    /// `X-Holon-Role`, which once made an editor or an admin an agent, changes nothing: whoever
    /// sends it is answered as they are, on every kind of route, and with no accounts too.
    #[test]
    fn a_role_header_changes_nothing() {
        let (access, [ada, bob, cy]) = server();
        let accounts = access.accounts().unwrap();
        accounts.add("claude", Kind::Agent, "agent", "ada", "ada").unwrap();
        let (claude, _) = accounts.create_token("claude", "Claude Code", "ada").unwrap();
        let local = accounts.new_local("bma");
        let said = |access: &Access, method: &str, url: &str, bearer: &str, header: Option<&str>| {
            let mut headers = vec![("Host".to_string(), "localhost:7420".to_string()), ("Authorization".to_string(), format!("Bearer {bearer}"))];
            headers.extend(header.map(|h| ("X-Holon-Role".to_string(), h.to_string())));
            access.admit(&Request::fake_http(method, url, headers, vec![])).map(|c| {
                let role = c.host_role();
                (c.name, c.kind, role)
            }).map_err(|r| r.status_code)
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
        // With no accounts, everyone is no one, let through but to decide an approval.
        let open = Access::default();
        for (method, url) in routes {
            assert_eq!(said(&open, method, url, "", Some("agent")), said(&open, method, url, "", None), "no accounts: {method} {url}");
        }
        assert_eq!(said(&open, "POST", "/deploy", "", Some("agent")), Ok((None, None, None)));
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
