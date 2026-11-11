//! Signing in, and the admin panel's API (accounts.rs, access.rs).
//!
//! Open: `GET /auth/state` (whether the server needs setting up, and whether signing in through a
//! provider is offered), `POST /auth/setup` (the first admin, with the setup code),
//! `POST /auth/sign-in`, `GET /auth/invite?token=` (whose link it is) and `POST /auth/invite`
//! (set a password with it), `GET /auth/config` (where to sign in through the provider).
//! Signed in: `GET /auth/me`, `GET /auth/owned` (the agents and services they own, with their
//! tokens: what an owner who isn't an admin makes and revokes tokens for), `POST /auth/password`,
//! `POST /auth/sign-out`, and `GET`/`POST /auth/prefs` (what the editor keeps for them: its home
//! tab, the welcome).
//! People (the design's §10, docs/L4s/access-and-approvals-L4.md), admins only:
//! - `GET /people`: every principal, with an agent's or a service's tokens;
//! - `POST /people` (`{name, kind, role, owner?, email?, display?}`): a person, answered with
//!   their invite link; an agent or a service, owned by `owner` (by default whoever asks);
//! - `PATCH /people/<name>` (role, disabled, email, display, owner), `DELETE /people/<name>`,
//!   `POST /people/<name>/reset` (a person's new invite link);
//! - `GET`/`POST /people/<name>/tokens` (`{label}`), `DELETE /people/<name>/tokens/<id>`: also for
//!   the agent's or service's owner.
//!
//! Admins also: `/admin/oidc` (read, set, turn off), `/admin/folders` (folders.rs). What an
//! admin does goes in the activity log.

use rouille::{Request, Response};
use serde::Deserialize;

use crate::access::{Caller, Role};
use crate::accounts::{Accounts, Kind, OidcSettings, Principal};
use crate::http::{err, read_body};
use crate::AppState;

fn body<T: for<'de> Deserialize<'de>>(request: &Request) -> Result<T, Response> {
    let bytes = read_body(request)?;
    serde_json::from_slice(&bytes).map_err(|e| err(400, format!("invalid body: {e}")))
}

/// A principal as the People page shows it: never its password hash; an agent's or a service's
/// tokens with it, never their hashes.
fn shown(p: &Principal, accounts: &Accounts) -> serde_json::Value {
    let owner_disabled = p.owner.as_deref().and_then(|o| accounts.principal(o)).is_some_and(|o| o.disabled);
    let mut v = serde_json::json!({
        "name": p.name, "kind": p.kind, "role": p.role, "display": p.display, "email": p.email,
        "disabled": p.disabled, "created_at": p.created_at, "created_by": p.created_by,
        "owner": p.owner, "owner_disabled": owner_disabled,
        "invited": accounts.invited(&p.name),
    });
    if p.kind != Kind::Person {
        v["tokens"] = serde_json::json!(accounts.tokens(&p.name));
    }
    v
}

fn signed_in(token: &str, p: &Principal) -> Response {
    Response::json(&serde_json::json!({ "token": token, "name": p.name, "kind": p.kind, "role": p.role }))
}

/// `/auth/…`, `/admin/…` and `/people…`, or None for any other path.
pub(crate) fn route(state: &AppState, request: &Request, caller: &Caller) -> Option<Response> {
    let url = request.url();
    if !url.starts_with("/auth/") && !url.starts_with("/admin/") && url != "/people" && !url.starts_with("/people/") {
        return None;
    }
    // The folders an admin binds (folders.rs): a server without accounts has them too.
    if let Some(response) = crate::folders::route(state, request, caller.name.as_deref().unwrap_or("")) {
        return Some(response);
    }
    // What the host stores, and how often it sweeps (storage.rs); the content store's sweep (sweep.rs).
    if let Some(response) = crate::storage::route(state, request, caller.name.as_deref().unwrap_or("")) {
        return Some(response);
    }
    if let Some(response) = crate::sweep::route(state, request, caller.name.as_deref().unwrap_or("")) {
        return Some(response);
    }
    // And the databases (databases.rs).
    if let Some(response) = crate::databases::route(state, request, caller.name.as_deref().unwrap_or("")) {
        return Some(response);
    }
    // And the mail resources (mails.rs).
    if let Some(response) = crate::mails::route(state, request, caller.name.as_deref().unwrap_or("")) {
        return Some(response);
    }
    // And the feeds (feeds.rs).
    if let Some(response) = crate::feeds::route(state, request, caller.name.as_deref().unwrap_or("")) {
        return Some(response);
    }
    // And the queues (queues.rs).
    if let Some(response) = crate::queues::route(state, request, caller.name.as_deref().unwrap_or("")) {
        return Some(response);
    }
    // Who holds what (grants.rs): a server without accounts has deployments too.
    if request.method() == "GET" && url == "/admin/grants" {
        return Some(crate::grants::handle_get_admin_grants(state));
    }
    // The variables an env secret can name (secret_sources.rs), for the deploy settings to offer:
    // names only, and only to an admin, who alone sets secrets (`/admin/…`, access.rs).
    if url == "/admin/secrets/env" && request.method() == "GET" {
        let (names, dotenv) = state.secrets.env_names();
        return Some(Response::json(&serde_json::json!({ "names": names, "dotenv": dotenv })));
    }
    // The server's own secrets, bound for every deployment that reads one (secrets.rs's SERVER).
    let by = caller.name.as_deref();
    if url == "/admin/secrets" {
        return Some(crate::deploy::handle_server_secrets(&state.secrets, &state.deployments, None, request.method(), request, by));
    }
    if let Some(name) = url.strip_prefix("/admin/secrets/") {
        return Some(crate::deploy::handle_server_secrets(&state.secrets, &state.deployments, Some(name), request.method(), request, by));
    }
    let Some(accounts) = state.access.accounts() else { return Some(err(404, "this server has no accounts")) };
    let by = caller.name.as_deref().unwrap_or("");
    // Whose what they add is, and what they own: the OS user by its name.
    let me = by.strip_prefix("local:").unwrap_or(by);
    let method = request.method();
    let respond = |r: Result<Response, Response>| Some(r.unwrap_or_else(|e| e));
    let fail = |e: String| err(400, e);
    Some(match (method, url.as_str()) {
        ("GET", "/auth/state") => {
            let oidc = accounts.oidc();
            Response::json(&serde_json::json!({ "setup": accounts.needs_setup(), "oidc": oidc.as_ref().map(|o| o.public_config()) }))
        }
        ("GET", "/auth/config") => match accounts.oidc().as_ref() {
            Some(oidc) => Response::json(&oidc.public_config()),
            None => err(404, "signing in through a provider isn't set on this server"),
        },
        ("POST", "/auth/setup") => {
            #[derive(Deserialize)]
            struct Setup {
                code: String,
                name: String,
                password: String,
            }
            return respond(body::<Setup>(request).and_then(|s| {
                let (token, a) = accounts.setup(&s.code, &s.name, &s.password).map_err(fail)?;
                state.activity.record("set up the server", &a.name, Some(&a.name));
                Ok(signed_in(&token, &a))
            }));
        }
        ("POST", "/auth/sign-in") => {
            #[derive(Deserialize)]
            struct SignIn {
                name: String,
                password: String,
            }
            return respond(body::<SignIn>(request).and_then(|s| {
                let from = request.remote_addr().ip().to_string();
                let (token, a) = accounts.sign_in_from(&s.name, &s.password, &from).map_err(|e| err(if e.contains("busy") { 503 } else { 401 }, e))?;
                Ok(signed_in(&token, &a))
            }));
        }
        ("GET", "/auth/invite") => {
            let token = request.get_param("token").unwrap_or_default();
            match accounts.invite_for(&token) {
                Some(name) => Response::json(&serde_json::json!({ "name": name })),
                None => err(404, "this link is used, expired or wrong: ask for a new one"),
            }
        }
        ("POST", "/auth/invite") => {
            #[derive(Deserialize)]
            struct Accept {
                token: String,
                password: String,
            }
            return respond(body::<Accept>(request).and_then(|s| {
                let (token, a) = accounts.accept(&s.token, &s.password).map_err(fail)?;
                state.activity.record("joined", &a.name, Some(&a.name));
                Ok(signed_in(&token, &a))
            }));
        }
        ("GET", "/auth/me") => match caller.name.as_deref().and_then(|n| accounts.principal(n)) {
            Some(p) => Response::json(&serde_json::json!({ "name": p.name, "kind": p.kind, "role": p.role, "email": p.email, "owner": p.owner, "display": p.display })),
            // The OS user, through the run file (`local:<user>`).
            None => Response::json(&serde_json::json!({ "name": by, "kind": caller.kind, "role": caller.role })),
        },
        ("GET", "/auth/owned") => Response::json(&accounts.principals().iter().filter(|p| p.owner.as_deref() == Some(me)).map(|p| shown(p, accounts)).collect::<Vec<_>>()),
        ("POST", "/auth/password") => {
            #[derive(Deserialize)]
            struct Change {
                current: String,
                new: String,
            }
            return respond(body::<Change>(request).and_then(|c| {
                let asking = request.header("Authorization").and_then(|h| h.strip_prefix("Bearer ")).map(str::trim);
                accounts.change_password(by, &c.current, &c.new, asking).map_err(fail)?;
                Ok(Response::empty_204())
            }));
        }
        ("GET", "/auth/prefs") => Response::json(&accounts.prefs(by)),
        ("POST", "/auth/prefs") => {
            return respond(body::<serde_json::Value>(request).and_then(|p| {
                accounts.set_prefs(by, p).map_err(fail)?;
                Ok(Response::empty_204())
            }));
        }
        ("POST", "/auth/sign-out") => {
            if let Some(token) = request.header("Authorization").and_then(|h| h.strip_prefix("Bearer ")) {
                accounts.sign_out(token.trim());
            }
            Response::empty_204()
        }

        // ── people (access.rs lets only admins this far, and owners to their principals' tokens) ──
        ("GET", "/people") => Response::json(&accounts.principals().iter().map(|p| shown(p, accounts)).collect::<Vec<_>>()),
        ("POST", "/people") => {
            #[derive(Deserialize)]
            struct New {
                name: String,
                #[serde(default)]
                kind: Kind,
                role: Role,
                #[serde(default)]
                owner: Option<String>,
                #[serde(default)]
                email: Option<String>,
                #[serde(default)]
                display: Option<String>,
            }
            return respond(body::<New>(request).and_then(|n| {
                let invite = if n.kind == Kind::Person {
                    let link = accounts.invite(&n.name, n.role, n.email, by).map_err(fail)?;
                    state.activity.record("invited", &n.name, Some(by));
                    Some(link)
                } else {
                    // Owned by whoever adds it, unless they name someone.
                    let owner = n.owner.unwrap_or_else(|| me.to_string());
                    accounts.add(&n.name, n.kind, n.role, &owner, by).map_err(fail)?;
                    state.activity.record(if n.kind == Kind::Agent { "added the agent" } else { "added the service" }, &n.name, Some(by));
                    None
                };
                if n.display.is_some() {
                    accounts.set_display(&n.name, n.display).map_err(fail)?;
                }
                Ok(Response::json(&match invite {
                    Some(link) => serde_json::json!({ "name": n.name, "invite": link }),
                    None => shown(&accounts.principal(&n.name).ok_or_else(|| err(404, "no such principal"))?, accounts),
                }))
            }));
        }
        ("GET", "/admin/oidc") => Response::json(&serde_json::json!({ "settings": accounts.oidc_settings() })),
        ("POST", "/admin/oidc") => {
            #[derive(Deserialize)]
            struct Set {
                #[serde(default)]
                settings: Option<OidcSettings>,
            }
            return respond(body::<Set>(request).and_then(|s| {
                let what = if s.settings.is_some() { "set signing in through a provider" } else { "turned off signing in through a provider" };
                let issuer = s.settings.as_ref().map(|o| o.issuer.clone()).unwrap_or_default();
                accounts.set_oidc(s.settings).map_err(fail)?;
                state.activity.record(what, &issuer, Some(by));
                Ok(Response::empty_204())
            }));
        }
        _ => {
            let Some(rest) = url.strip_prefix("/people/") else { return Some(Response::empty_404()) };
            if let Some((name, tokens)) = rest.split_once('/')
                && (tokens == "tokens" || tokens.starts_with("tokens/"))
            {
                // An admin's, or the owner's (access.rs lets a person this far for any name).
                if caller.role != Some(Role::Admin) && accounts.principal(name).and_then(|p| p.owner).as_deref() != Some(me) {
                    return Some(err(403, format!("{by} may not do that: an agent's or a service's tokens are its owner's or an admin's to make")));
                }
                return respond(match (method, tokens.strip_prefix("tokens/")) {
                    ("GET", None) => Ok(Response::json(&accounts.tokens(name))),
                    ("POST", None) => {
                        #[derive(Deserialize)]
                        struct New {
                            label: String,
                        }
                        body::<New>(request).and_then(|n| {
                            let (token, entry) = accounts.create_token(name, &n.label, by).map_err(fail)?;
                            state.activity.record("made a token for", name, Some(by));
                            Ok(Response::json(&serde_json::json!({ "token": token, "entry": entry })))
                        })
                    }
                    ("DELETE", Some(id)) => accounts.revoke_token(name, id).map_err(|e| err(404, e)).map(|_| {
                        state.activity.record("revoked a token of", name, Some(by));
                        Response::empty_204()
                    }),
                    _ => Ok(Response::empty_404()),
                });
            }
            if let Some(name) = rest.strip_suffix("/reset")
                && method == "POST"
            {
                return respond(accounts.reset(name).map_err(|e| err(if e.starts_with("no principal") { 404 } else { 400 }, e)).map(|link| {
                    state.activity.record("reset the password of", name, Some(by));
                    Response::json(&serde_json::json!({ "name": name, "invite": link }))
                }));
            }
            let name = rest;
            if name.contains('/') {
                return Some(Response::empty_404());
            }
            if method == "DELETE" {
                return respond(accounts.remove(name).map_err(fail).map(|_| {
                    state.activity.record("removed", name, Some(by));
                    Response::empty_204()
                }));
            }
            if method == "PATCH" {
                #[derive(Deserialize)]
                struct Change {
                    role: Option<Role>,
                    disabled: Option<bool>,
                    /// `""` clears it.
                    email: Option<String>,
                    /// `""` clears it.
                    display: Option<String>,
                    owner: Option<String>,
                }
                return respond(body::<Change>(request).and_then(|c| {
                    if let Some(role) = c.role {
                        accounts.set_role(name, role).map_err(fail)?;
                        state.activity.record(&format!("made {}", serde_json::to_value(role).unwrap().as_str().unwrap_or("")), name, Some(by));
                    }
                    if let Some(disabled) = c.disabled {
                        accounts.set_disabled(name, disabled).map_err(fail)?;
                        state.activity.record(if disabled { "disabled" } else { "enabled" }, name, Some(by));
                    }
                    if let Some(email) = c.email {
                        accounts.set_email(name, Some(email)).map_err(fail)?;
                    }
                    if let Some(display) = c.display {
                        accounts.set_display(name, Some(display)).map_err(fail)?;
                    }
                    if let Some(owner) = c.owner {
                        accounts.set_owner(name, &owner).map_err(fail)?;
                        state.activity.record(&format!("made {owner} the owner of"), name, Some(by));
                    }
                    let p = accounts.principal(name).ok_or_else(|| err(404, "no such principal"))?;
                    Ok(Response::json(&shown(&p, accounts)))
                }));
            }
            Response::empty_404()
        }
    })
}
