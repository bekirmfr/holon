//! Signing in, and the admin panel's API (accounts.rs; who may ask what is can.rs's table).
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
//! - `GET /people`: every principal, with what is bound to it (bindings.rs) and its `role`, the
//!   widest built-in role bound to it on the host, and with an agent's or a service's tokens;
//! - `POST /people` (`{name, kind, role, owner?, email?, display?}`): a person, answered with
//!   their invite link; an agent or a service, owned by `owner` (by default whoever asks); `role`
//!   is bound to it on the host; 409 for a role that would wait for people (access_proposals.rs
//!   `adding`): a new principal is made with a role that gives less, and given it after;
//! - `PATCH /people/<name>` (role, disabled, email, display, owner; a role is bound on the host in
//!   place of what was there, and a role that gives more, while the host has people approve such
//!   a change, waits: 202, the principal with the `proposal`), `DELETE /people/<name>` (with what
//!   is bound to it),
//!   `POST /people/<name>/reset` (a person's new invite link);
//! - `GET`/`POST /people/<name>/tokens` (`{label}`), `DELETE /people/<name>/tokens/<id>`: also for
//!   the agent's or service's owner (can.rs's owner rule).
//!
//! Admins also: `/admin/oidc` (read, set, turn off), `/admin/folders` (folders.rs). What an
//! admin does goes in the activity log.
//!
//! No one changes someone holding more than they do (D19, `may_change`). And while an access
//! change that gives more waits for people, no one makes a way in to someone else holding what
//! that governs, whose name would count as a second person (the access plan's D12,
//! `may_take_over`): a reset, their email, a token of theirs but by its owner, their owner, the
//! provider. They do it themselves, or this machine's run file does; or what they hold is taken
//! away first, which applies at once, and given back after, which waits.

use rouille::{Request, Response};
use serde::Deserialize;

use crate::access::Caller;
use crate::access_proposals::{govern, Governed};
use crate::accounts::{Accounts, Kind, OidcSettings, Principal};
use crate::http::{err, read_body};
use crate::AppState;

fn body<T: for<'de> Deserialize<'de>>(request: &Request) -> Result<T, Response> {
    let bytes = read_body(request)?;
    serde_json::from_slice(&bytes).map_err(|e| err(400, format!("invalid body: {e}")))
}

/// What holds for `name`: each binding's id, role and what it allows, scope, the group it holds
/// through if it does, who bound it when, and until when, with whether it has expired. A principal
/// reads its own here (`/auth/me`), what its roles allow included, where `GET /roles` is an
/// admin's.
fn bindings(name: &str, accounts: &Accounts) -> serde_json::Value {
    let now = crate::deploy::epoch_now();
    let document = accounts.access_document();
    let each = |(b, group): &(crate::bindings::Binding, Option<String>)| {
        let allows: Vec<&str> = document.role(&b.role).map(|r| r.allows().into_iter().map(crate::can::Action::name).collect()).unwrap_or_default();
        serde_json::json!({ "id": b.id, "role": b.role, "allows": allows, "scope": b.scope, "group": group, "by": b.by, "at": b.at, "expires_at": b.expires_at, "expired": !b.live(now) })
    };
    accounts.reaching(name).iter().map(each).collect()
}

/// A principal as the People page shows it: what is bound to it, its role on the host, and when it
/// was last seen; never its password hash; an agent's or a service's tokens with it, each with when
/// it was last used, never their hashes.
fn shown(p: &Principal, accounts: &Accounts) -> serde_json::Value {
    let owner_disabled = p.owner.as_deref().and_then(|o| accounts.principal(o)).is_some_and(|o| o.disabled);
    let mut v = serde_json::json!({
        "name": p.name, "kind": p.kind, "role": accounts.host_role(&p.name), "bindings": bindings(&p.name, accounts),
        "display": p.display, "email": p.email,
        "disabled": p.disabled, "created_at": p.created_at, "created_by": p.created_by, "last_seen": p.last_seen,
        "owner": p.owner, "owner_disabled": owner_disabled,
        "invited": accounts.invited(&p.name),
        "ways_in": accounts.acting_as(&p.name).into_iter().map(|(_, how)| how).collect::<Vec<_>>(),
    });
    if p.kind != Kind::Person {
        v["tokens"] = serde_json::json!(accounts.tokens(&p.name));
    }
    v
}

/// Why `caller` may not give the role named `role` on the host, as inviting, adding or setting
/// someone with it binds it there, if they may not: 400 for no such role. A binding, so as
/// `POST /bindings` asks: people.assign on the host (people.manage, which lets them this far,
/// makes and changes principals, not what they hold), and what it gives, for good (D19).
fn may_give(caller: &Caller, accounts: &Accounts, role: &str) -> Result<(), Response> {
    let Some(def) = accounts.access_document().role(role) else { return Err(err(400, format!("no role {role:?}: GET /roles lists them"))) };
    if crate::can::can(caller, crate::can::Action::PeopleAssign, &crate::can::Resource::Host).is_err() {
        return Err(err(403, format!("giving {role} binds it on the host, which takes people.assign there, which you don't hold")));
    }
    crate::bindings::may_bind_until(caller, &def, &crate::bindings::Scope::Host, None)
}

/// What `name` holds that `caller` doesn't, in words, if anything (D19): or what an agent or a
/// service `name` owns holds, whose tokens whoever acts as `name` makes.
fn holds_more(caller: &Caller, accounts: &Accounts, name: &str) -> Option<String> {
    let document = accounts.access_document();
    let more = |who: &str| -> Option<String> {
        let p = accounts.principal(who)?;
        let (b, a) = crate::bindings::beyond(caller, &document, p.kind, document.reaching(who).map(|(b, _)| b), crate::deploy::epoch_now(), true)?;
        Some(format!("{} {}, which holds {} there, which you don't", b.role, b.scope.said(), a.name()))
    };
    match more(name) {
        Some(more) => Some(format!("{name} holds {more}")),
        None => accounts.owned(name).iter().find_map(|o| Some(format!("{name} owns {o}, which holds {}", more(o)?))),
    }
}

/// Why `caller` may not change `name`, if they may not: its sign-in (a reset, its email), its
/// tokens, its owner, its role, whether it is enabled, or that it is there. Whoever does could act
/// as `name` or take from them, so they hold everything `name` holds, as they would to have given
/// it (D19, both ways).
pub(crate) fn may_change(caller: &Caller, accounts: &Accounts, name: &str) -> Result<(), Response> {
    match holds_more(caller, accounts, name) {
        Some(more) => Err(err(403, format!("{more}: you change, or act as, only someone who holds no more than you"))),
        None => Ok(()),
    }
}

/// Why `caller` may not do `what` ("resets bob's password"), which makes a way in to `name`, if
/// they may not (the access plan's D12): while this host has people approve an access change that
/// gives more, whoever made one to someone holding what that governs could act as them, counted as
/// another person, so no one makes one but `name` themself and this machine's run file, which
/// holds the data folder anyway. Taken from them first, which applies at once, it is given back
/// after, which waits.
fn may_take_over(caller: &Caller, accounts: &Accounts, name: &str, what: &str) -> Result<(), Response> {
    if caller.local.is_some() || caller.name.as_deref() == Some(name) {
        return Ok(());
    }
    let Some(n) = accounts.access_document().approvals.changes else { return Ok(()) };
    let Some(held) = crate::access_proposals::governing_as(accounts, name) else { return Ok(()) };
    let instead = format!("or take that from {} first, which applies at once, and give it back after, which waits", held.0);
    Err(err(403, crate::access_proposals::taken_over(name, &held, what, n, &instead)))
}

/// A session begun: its token, whose it is, and when it ends (`holon login` says so).
fn signed_in(token: &str, p: &Principal, accounts: &Accounts) -> Response {
    Response::json(&serde_json::json!({
        "token": token, "name": p.name, "kind": p.kind, "role": accounts.host_role(&p.name), "expires_at": accounts.session_ends(token),
    }))
}

/// `/auth/…`, `/admin/…` and `/people…`, or None for any other path.
pub(crate) fn route(state: &AppState, request: &Request, caller: &Caller) -> Option<Response> {
    let url = request.url();
    if !url.starts_with("/auth/") && !url.starts_with("/admin/") && url != "/people" && !url.starts_with("/people/") {
        return None;
    }
    // Where a folder, a database, a mail, a feed or a queue is, only to whoever may bind one (web
    // console L4, Q4).
    let sees_where = crate::can::sees_where(caller);
    // The folders an admin binds (folders.rs): a server without accounts has them too.
    if let Some(response) = crate::folders::route(state, request, caller.name.as_deref().unwrap_or(""), sees_where) {
        return Some(response);
    }
    // What the host stores, and how often it sweeps (storage.rs); the content store's sweep (sweep.rs).
    if let Some(response) = crate::storage::route(state, request, caller.name.as_deref().unwrap_or("")) {
        return Some(response);
    }
    if let Some(response) = crate::sweep::route(state, request, caller.name.as_deref().unwrap_or("")) {
        return Some(response);
    }
    // The keys the server seals its data under (keys.rs).
    if let Some(response) = crate::keys::route(state, request, caller.name.as_deref().unwrap_or("")) {
        return Some(response);
    }
    // And the databases (databases.rs).
    if let Some(response) = crate::databases::route(state, request, caller.name.as_deref().unwrap_or(""), sees_where) {
        return Some(response);
    }
    // And the mail resources (mails.rs).
    if let Some(response) = crate::mails::route(state, request, caller.name.as_deref().unwrap_or(""), sees_where) {
        return Some(response);
    }
    // And the feeds (feeds.rs).
    if let Some(response) = crate::feeds::route(state, request, caller.name.as_deref().unwrap_or(""), sees_where) {
        return Some(response);
    }
    // And the queues (queues.rs).
    if let Some(response) = crate::queues::route(state, request, caller.name.as_deref().unwrap_or(""), sees_where) {
        return Some(response);
    }
    // Who holds what (grants.rs): a server without accounts has deployments too.
    if request.method() == "GET" && url == "/admin/grants" {
        return Some(crate::grants::handle_get_admin_grants(state));
    }
    // The variables an env secret can name (secret_sources.rs), for the deploy settings to offer:
    // names only, and only to an admin, who alone sets secrets (`/admin/…`, can.rs).
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
    // Whose what they add is, and what they own: the OS user as `local:<user>`, which no principal
    // is named (accounts.rs, `may_own`).
    let me = by;
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
                let (token, a) = accounts.setup_from(&s.code, &s.name, &s.password, &state.access.client(request)).map_err(|e| err(if e.contains("busy") { 503 } else { 400 }, e))?;
                state.activity.record("set up the server", &a.name, Some(&a.name));
                Ok(signed_in(&token, &a, accounts))
            }));
        }
        ("POST", "/auth/sign-in") => {
            #[derive(Deserialize)]
            struct SignIn {
                name: String,
                password: String,
            }
            return respond(body::<SignIn>(request).and_then(|s| {
                let (token, a) = accounts.sign_in_from(&s.name, &s.password, &state.access.client(request)).map_err(|e| err(if e.contains("busy") { 503 } else { 401 }, e))?;
                Ok(signed_in(&token, &a, accounts))
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
                Ok(signed_in(&token, &a, accounts))
            }));
        }
        // Through the run file, `local` says so: the OS user, `local:<user>` (D45). `access_version`
        // is the access document's: what `can` answers changes only with it (or a flow's tags), so
        // the editor reads its buttons again when it moves.
        ("GET", "/auth/me") => {
            let access_version = accounts.access_document().version;
            match caller.name.as_deref().and_then(|n| accounts.principal(n)) {
                Some(p) => Response::json(&serde_json::json!({
                    "name": p.name, "kind": p.kind, "role": caller.host_role(), "bindings": bindings(&p.name, accounts),
                    "email": p.email, "owner": p.owner, "display": p.display, "local": caller.local, "access_version": access_version,
                })),
                // The run file tied to no account (`local:<user>`): admin on the host, bound nowhere.
                None => Response::json(&serde_json::json!({ "name": by, "kind": caller.kind, "role": caller.host_role(), "local": caller.local, "access_version": access_version })),
            }
        }
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
        // 204 only when the sign-in it came with ended: anything else says why it goes on, or
        // that it had ended (the gate let it in a moment before).
        ("POST", "/auth/sign-out") => {
            let token = request.header("Authorization").and_then(|h| h.strip_prefix("Bearer ")).map(str::trim).unwrap_or_default();
            if accounts.local(token).is_some() {
                return Some(err(409, "this is the machine's run file, which signs in while the server runs: it ends when the server stops"));
            }
            if token.starts_with("ht_") {
                return Some(err(409, "an API token signs no one in to sign out of: revoke it to end it (on the People page, or holon people revoke <name> <id>)"));
            }
            // A provider's ID token is good until it expires: it is kept refused until then.
            let ended = if crate::oidc::looks_like_jwt(token) {
                match accounts.oidc().as_ref().and_then(|oidc| oidc.verify(token).ok()).map(|identity| identity.expires_at) {
                    Some(until) => accounts.end_provider_sign_in(token, until).map(|()| true),
                    None => Ok(false),
                }
            } else {
                accounts.sign_out(token)
            };
            match ended {
                Ok(true) => Response::empty_204(),
                Ok(false) => err(401, "your sign-in had ended already"),
                Err(e) => err(500, format!("couldn't sign you out: {e}: you are still signed in")),
            }
        }

        // ── people (the gate lets only admins this far, and owners to their principals' tokens) ──
        ("GET", "/people") => Response::json(&accounts.principals().iter().map(|p| shown(p, accounts)).collect::<Vec<_>>()),
        ("POST", "/people") => {
            #[derive(Deserialize)]
            struct New {
                name: String,
                #[serde(default)]
                kind: Kind,
                /// A built-in role's name, or one an admin defined.
                role: String,
                #[serde(default)]
                owner: Option<String>,
                #[serde(default)]
                email: Option<String>,
                #[serde(default)]
                display: Option<String>,
            }
            return respond(body::<New>(request).and_then(|n| {
                may_give(caller, accounts, &n.role)?;
                crate::access_proposals::adding(accounts, &n.name, n.kind, &n.role).map_err(|e| err(409, e))?;
                let invite = if n.kind == Kind::Person {
                    let link = accounts.invite(&n.name, &n.role, n.email, by).map_err(fail)?;
                    state.activity.record("invited", &n.name, Some(by));
                    Some(link)
                } else {
                    // Owned by whoever adds it, unless they name someone.
                    let owner = n.owner.unwrap_or_else(|| me.to_string());
                    accounts.add(&n.name, n.kind, &n.role, &owner, by).map_err(fail)?;
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
                // The provider says who each person here is: whoever sets it could be any of them.
                if let Some(more) = accounts.principals().iter().filter(|p| p.kind == Kind::Person).find_map(|p| holds_more(caller, accounts, &p.name)) {
                    return Err(err(403, format!("{more}: a provider says who each person here is, so it is set only by someone who holds what each of them holds")));
                }
                // And while giving waits for people, it is a way in to each of them (D12).
                let others = || accounts.principals().into_iter().filter(|p| p.kind == Kind::Person && Some(p.name.as_str()) != caller.name.as_deref());
                if s.settings.is_some()
                    && caller.local.is_none()
                    && let Some(n) = accounts.access_document().approvals.changes
                    && let Some((name, held)) = others().find_map(|p| Some((p.name.clone(), crate::access_proposals::governing_as(accounts, &p.name)?)))
                {
                    let instead = "or turn off access changes waiting for people first (holon access approvals --changes off), which waits too";
                    return Err(err(403, crate::access_proposals::taken_over(&name, &held, "sets the identity provider, which says who each person here is,", n, instead)));
                }
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
                // Its owner makes and revokes its tokens, as the design has it; anyone else only
                // holding what it holds.
                let owned = accounts.principal(name).and_then(|p| p.owner).as_deref() == Some(me);
                return respond(match (method, tokens.strip_prefix("tokens/")) {
                    ("GET", None) => Ok(Response::json(&accounts.tokens(name))),
                    ("POST", None) => {
                        #[derive(Deserialize)]
                        struct New {
                            label: String,
                        }
                        body::<New>(request).and_then(|n| {
                            if !owned {
                                may_change(caller, accounts, name)?;
                                may_take_over(caller, accounts, name, &format!("makes a token of {name}"))?;
                            }
                            let (token, entry) = accounts.create_token(name, &n.label, by).map_err(fail)?;
                            state.activity.record("made a token for", name, Some(by));
                            Ok(Response::json(&serde_json::json!({ "token": token, "entry": entry })))
                        })
                    }
                    ("DELETE", Some(id)) => (if owned { Ok(()) } else { may_change(caller, accounts, name) }).and_then(|_| {
                        accounts.revoke_token(name, id).map_err(|e| err(404, e)).map(|_| {
                            state.activity.record("revoked a token of", name, Some(by));
                            Response::empty_204()
                        })
                    }),
                    _ => Ok(Response::empty_404()),
                });
            }
            if let Some(name) = rest.strip_suffix("/reset")
                && method == "POST"
            {
                if let Err(r) = may_change(caller, accounts, name).and_then(|_| may_take_over(caller, accounts, name, &format!("resets {name}'s password"))) {
                    return Some(r);
                }
                return respond(accounts.reset(name, by).map_err(|e| err(if e.starts_with("no principal") { 404 } else { 400 }, e)).map(|link| {
                    state.activity.record("reset the password of", name, Some(by));
                    Response::json(&serde_json::json!({ "name": name, "invite": link }))
                }));
            }
            let name = rest;
            if name.contains('/') {
                return Some(Response::empty_404());
            }
            if matches!(method, "DELETE" | "PATCH")
                && let Err(r) = may_change(caller, accounts, name)
            {
                return Some(r);
            }
            if method == "DELETE" {
                return respond(accounts.remove(name, by).map_err(fail).map(|_| {
                    state.activity.record("removed", name, Some(by));
                    Response::empty_204()
                }));
            }
            if method == "PATCH" {
                #[derive(Deserialize)]
                struct Change {
                    role: Option<String>,
                    disabled: Option<bool>,
                    /// `""` clears it.
                    email: Option<String>,
                    /// `""` clears it.
                    display: Option<String>,
                    owner: Option<String>,
                }
                return respond(body::<Change>(request).and_then(|c| {
                    // A way in to someone holding what waits for people to be given (D12), before
                    // anything is changed: another email, which a provider matches, or owner.
                    let now = accounts.principal(name);
                    if let Some(email) = c.email.as_deref().map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty())
                        && now.as_ref().is_some_and(|p| p.email.as_deref() != Some(email.as_str()))
                    {
                        may_take_over(caller, accounts, name, &format!("sets {name}'s email"))?;
                    }
                    if let Some(owner) = c.owner.as_deref().filter(|o| now.as_ref().is_some_and(|p| p.owner.as_deref() != Some(*o))) {
                        may_take_over(caller, accounts, name, &format!("makes {owner} the owner of {name}"))?;
                    }
                    // A role that gives more waits for people (access_proposals.rs); the rest is
                    // changed at once, and the answer says what waits.
                    let mut waits = None;
                    if let Some(role) = c.role {
                        may_give(caller, accounts, &role)?;
                        let change = crate::access_proposals::Change::HostRole { name: name.into(), role: role.clone() };
                        match govern(accounts, caller, change, None, || accounts.set_role(name, &role, by, None)).map_err(fail)? {
                            Governed::Made(()) => state.activity.record(&format!("made {role}"), name, Some(by)),
                            Governed::Waits(p) => {
                                state.activity.record("proposed an access change", &format!("{}: {}", p.id, p.what), Some(by));
                                waits = Some(p);
                            }
                        }
                    }
                    if let Some(disabled) = c.disabled {
                        accounts.set_disabled(name, disabled).map_err(fail)?;
                        state.activity.record(if disabled { "disabled" } else { "enabled" }, name, Some(by));
                    }
                    if let Some(email) = c.email {
                        accounts.set_email(name, Some(email), by).map_err(fail)?;
                    }
                    if let Some(display) = c.display {
                        accounts.set_display(name, Some(display)).map_err(fail)?;
                    }
                    if let Some(owner) = c.owner {
                        accounts.set_owner(name, &owner).map_err(fail)?;
                        state.activity.record(&format!("made {owner} the owner of"), name, Some(by));
                    }
                    let p = accounts.principal(name).ok_or_else(|| err(404, "no such principal"))?;
                    let mut answer = shown(&p, accounts);
                    Ok(match waits {
                        Some(proposal) => {
                            answer["proposal"] = serde_json::json!(proposal);
                            Response::json(&answer).with_status_code(202)
                        }
                        None => Response::json(&answer),
                    })
                }));
            }
            Response::empty_404()
        }
    })
}
