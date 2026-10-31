//! Signing in, and the admin panel's API (accounts.rs, access.rs).
//!
//! Open: `GET /auth/state` (whether the server needs setting up, and whether signing in through a
//! provider is offered), `POST /auth/setup` (the first admin, with the setup code),
//! `POST /auth/sign-in`, `GET /auth/invite?token=` (whose link it is) and `POST /auth/invite`
//! (set a password with it), `GET /auth/config` (where to sign in through the provider).
//! Signed in: `GET /auth/me`, `POST /auth/password`, `POST /auth/sign-out`, and
//! `GET`/`POST /auth/prefs` (what the editor keeps for them: its home tab, the welcome).
//! Admins: `/admin/users` (list, invite), `/admin/users/<name>` (role, disabled, email; delete),
//! `/admin/users/<name>/reset`, `/admin/tokens` (list, create), `/admin/tokens/<id>` (revoke),
//! `/admin/oidc` (read, set, turn off), `/admin/folders` (folders.rs). What an admin does goes in
//! the activity log.

use rouille::{Request, Response};
use serde::Deserialize;

use crate::access::{Caller, Role};
use crate::accounts::{Account, Accounts, OidcSettings};
use crate::http::{err, read_body};
use crate::AppState;

fn body<T: for<'de> Deserialize<'de>>(request: &Request) -> Result<T, Response> {
    let bytes = read_body(request)?;
    serde_json::from_slice(&bytes).map_err(|e| err(400, format!("invalid body: {e}")))
}

/// An account as the panel shows it: never its password hash.
fn shown(a: &Account, accounts: &Accounts) -> serde_json::Value {
    serde_json::json!({
        "name": a.name, "role": a.role, "email": a.email, "disabled": a.disabled,
        "created_at": a.created_at, "created_by": a.created_by,
        "invited": accounts.invited(&a.name),
    })
}

fn signed_in(token: &str, a: &Account) -> Response {
    Response::json(&serde_json::json!({ "token": token, "name": a.name, "role": a.role }))
}

/// `/auth/…` and `/admin/…`, or None for any other path.
pub(crate) fn route(state: &AppState, request: &Request, caller: &Caller) -> Option<Response> {
    let url = request.url();
    if !url.starts_with("/auth/") && !url.starts_with("/admin/") {
        return None;
    }
    // The folders an admin binds (folders.rs): a server without accounts has them too.
    if let Some(response) = crate::folders::route(state, request, caller.name.as_deref().unwrap_or("")) {
        return Some(response);
    }
    // The content store's sweep (sweep.rs).
    if let Some(response) = crate::sweep::route(state, request, caller.name.as_deref().unwrap_or("")) {
        return Some(response);
    }
    // And the databases (databases.rs).
    if let Some(response) = crate::databases::route(state, request, caller.name.as_deref().unwrap_or("")) {
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
    let Some(accounts) = state.access.accounts() else { return Some(err(404, "this server has no accounts")) };
    let by = caller.name.as_deref().unwrap_or("");
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
                let (token, a) = accounts.sign_in(&s.name, &s.password).map_err(|e| err(401, e))?;
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
        ("GET", "/auth/me") => match caller.name.as_deref().and_then(|n| accounts.account(n)) {
            Some(a) => Response::json(&serde_json::json!({ "name": a.name, "role": a.role, "email": a.email })),
            // An API token, or signed in through a provider as an account by email.
            None => Response::json(&serde_json::json!({ "name": by, "role": caller.role })),
        },
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

        // ── the admin panel (access.rs lets only admins this far) ──
        ("GET", "/admin/users") => Response::json(&accounts.users().iter().map(|a| shown(a, accounts)).collect::<Vec<_>>()),
        ("POST", "/admin/users") => {
            #[derive(Deserialize)]
            struct Invite {
                name: String,
                role: Role,
                #[serde(default)]
                email: Option<String>,
            }
            return respond(body::<Invite>(request).and_then(|i| {
                let link = accounts.invite(&i.name, i.role, i.email, by).map_err(fail)?;
                state.activity.record("invited", &i.name, Some(by));
                Ok(Response::json(&serde_json::json!({ "name": i.name, "invite": link })))
            }));
        }
        ("GET", "/admin/tokens") => Response::json(&accounts.tokens()),
        ("POST", "/admin/tokens") => {
            #[derive(Deserialize)]
            struct New {
                label: String,
                role: Role,
            }
            return respond(body::<New>(request).and_then(|n| {
                let (token, entry) = accounts.create_token(&n.label, n.role, by).map_err(fail)?;
                state.activity.record("made an API token", &n.label, Some(by));
                Ok(Response::json(&serde_json::json!({ "token": token, "entry": entry })))
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
            if let Some(id) = url.strip_prefix("/admin/tokens/")
                && method == "DELETE"
            {
                return respond(accounts.revoke_token(id).map_err(|e| err(404, e)).map(|_| {
                    state.activity.record("revoked an API token", id, Some(by));
                    Response::empty_204()
                }));
            }
            if let Some(rest) = url.strip_prefix("/admin/users/") {
                if let Some(name) = rest.strip_suffix("/reset")
                    && method == "POST"
                {
                    return respond(accounts.reset(name).map_err(|e| err(404, e)).map(|link| {
                        state.activity.record("reset the password of", name, Some(by));
                        Response::json(&serde_json::json!({ "name": name, "invite": link }))
                    }));
                }
                let name = rest;
                if method == "DELETE" {
                    return respond(accounts.remove(name).map_err(fail).map(|_| {
                        state.activity.record("removed", name, Some(by));
                        Response::empty_204()
                    }));
                }
                if method == "POST" {
                    #[derive(Deserialize)]
                    struct Change {
                        role: Option<Role>,
                        disabled: Option<bool>,
                        /// `""` clears it.
                        email: Option<String>,
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
                        let a = accounts.account(name).ok_or_else(|| err(404, "no such account"))?;
                        Ok(Response::json(&shown(&a, accounts)))
                    }));
                }
            }
            Response::empty_404()
        }
    })
}
