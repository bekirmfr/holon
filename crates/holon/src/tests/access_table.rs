//! Who is let through, route by route (docs/L4s/can-and-catalogue-plan.md, step 2a): every route
//! the server answers, sent by every kind of caller, and what each is answered: let through, or
//! refused with which status and words. What a caller is answered comes from the table here and
//! `expected`, written from what each route lets today, never from can.rs: the same table
//! judges whatever decides in its place.

use super::*;
use crate::accounts::{Accounts, Kind};
use access::Role;
use serde_json::{json, Value};

const PW: &str = "correct horse battery";
const ADMINS: &str = "that needs an admin";
const AGENTS: &str = "an agent proposes and calls; approving, granting, starting, stopping and deleting are a person's";
const VIEWERS: &str = "a viewer only reads";
const OWNERS: &str = "an agent's or a service's tokens are its owner's or an admin's to make";
const NO_ACCOUNTS: &str = "deciding an approval needs a person signed in to Holon, and this server has no accounts";

/// Whom a route lets through.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Today {
    /// Anyone, signed in or not.
    Open,
    /// Anyone signed in: what is their own.
    SignedIn,
    /// Every role.
    All,
    /// Editors and admins.
    Editors,
    /// Editors, admins and agents.
    EditorsAndAgents,
    /// Admins.
    Admins,
    /// Admins, and the person who owns the principal (named here) whose tokens it is.
    Owners(&'static str),
    /// Editors and admins who are people: who decides an approval (durable.rs's `decider`).
    Deciders,
}

/// Who asks: a principal by its token, or no one.
struct Caller {
    name: &'static str,
    token: Option<String>,
    kind: Option<Kind>,
    role: Option<Role>,
    disabled: bool,
}

/// One request: a route, sent so that a caller let through changes nothing (a target that isn't
/// there, a body its handler refuses).
struct Row {
    method: &'static str,
    path: String,
    body: &'static str,
    today: Today,
    /// Whether a router answers it: a path no route has isn't.
    routed: bool,
}

/// What `c` is answered on a route that lets `today` through: None when it is let through, or the
/// refusal's status and words.
fn expected(today: Today, c: &Caller) -> Option<(u16, String)> {
    if today == Today::Open {
        return None;
    }
    if c.token.is_none() {
        return Some((401, "sign in first".into()));
    }
    if c.disabled {
        return Some((401, "your sign-in has ended or isn't valid: sign in again".into()));
    }
    let (kind, role) = (c.kind.unwrap(), c.role.unwrap());
    let refused = |words: &str| Some((403, format!("{} may not do that: {words}", c.name)));
    match today {
        Today::Open | Today::SignedIn | Today::All => None,
        _ if role == Role::Admin => None,
        Today::Admins => refused(ADMINS),
        Today::Owners(owner) if kind == Kind::Person && owner == c.name => None,
        Today::Owners(_) if kind == Kind::Person => refused(OWNERS),
        Today::Owners(_) => refused(ADMINS),
        Today::EditorsAndAgents if role == Role::Agent => None,
        _ if role == Role::Agent => refused(AGENTS),
        _ if role == Role::Viewer => refused(VIEWERS),
        Today::Deciders if kind == Kind::Service => Some((403, format!("{} is a service, a script, not a person: sign in to decide an approval", c.name))),
        Today::Editors | Today::EditorsAndAgents | Today::Deciders => None,
    }
}

/// Every route, as §2 of the plan lists them; `d` is a deployment of `unit` with the action
/// `echo`, `f` a saved flow; `none` and `nobody` aren't there.
fn rows(unit: &str) -> Vec<Row> {
    use Today::*;
    let at = |method, path: &str, body, today| Row { method, path: path.to_string(), body, today, routed: true };
    let unrouted = |method, path: &str, today| Row { method, path: path.to_string(), body: "", today, routed: false };
    let mut rows = vec![
        at("OPTIONS", "/flows", "", Open),
        at("GET", "/health", "", Open),
        at("GET", "/auth/state", "", Open),
        at("GET", "/auth/config", "", Open),
        at("GET", "/auth/invite", "", Open),
        at("POST", "/auth/setup", "[]", Open),
        at("POST", "/auth/sign-in", "[]", Open),
        at("POST", "/auth/invite", "[]", Open),
        at("GET", "/auth/me", "", SignedIn),
        at("GET", "/auth/owned", "", SignedIn),
        at("GET", "/auth/prefs", "", SignedIn),
        at("GET", "/version", "", SignedIn),
        at("POST", "/auth/password", "[]", SignedIn),
        at("POST", "/auth/prefs", "[]", SignedIn),
        // Flows and deployments.
        at("GET", "/flows", "", All),
        at("POST", "/flows", "[]", EditorsAndAgents),
        at("GET", "/flows/f", "", All),
        at("DELETE", "/flows/none", "", Editors),
        at("GET", &format!("/unit/{unit}"), "", All),
        at("POST", "/unit", "", EditorsAndAgents),
        at("POST", "/deploy/preview", "[]", EditorsAndAgents),
        at("POST", "/deploy", "[]", EditorsAndAgents),
        at("GET", "/proposals", "", All),
        at("GET", "/proposals/1", "", All),
        at("POST", "/proposals/1/decline", "[]", Editors),
        at("GET", "/deployments", "", All),
        at("POST", "/deployments/none/grant", "[]", Editors),
        at("POST", "/deployments/none/settings", "[]", Editors),
        at("POST", "/deployments/none/tick", "", Editors),
        at("POST", "/deployments/none/restore", "[]", EditorsAndAgents),
        at("DELETE", "/deployments/none", "", Editors),
        at("POST", "/trigger/d", "[]", Editors),
        // Runs and state.
        at("GET", "/deployments/none/logs", "", All),
        at("POST", "/deployments/none/request", "[]", EditorsAndAgents),
        // An action `d` hasn't: a deployment that isn't there is a bare 404.
        at("POST", "/trigger/d/x", "[]", EditorsAndAgents),
        at("POST", "/deployments/none/runs/r/decide", "[]", Deciders),
        at("GET", "/summary", "", All),
        at("GET", "/activity", "", All),
        // Secrets, the policy and the host.
        at("GET", "/deployments/d/secrets", "", All),
        at("GET", "/deployments/d/secrets/k", "", All),
        at("POST", "/deployments/none/secrets/k", "[]", Admins),
        at("DELETE", "/deployments/none/secrets/k", "", Admins),
        at("POST", "/deployments/none/secrets/k/rollback", "[]", Admins),
        at("GET", "/admin/secrets/env", "", Admins),
        at("GET", "/admin/secrets", "", Admins),
        at("GET", "/admin/secrets/k", "", Admins),
        at("POST", "/admin/secrets/k", "[]", Admins),
        at("POST", "/admin/secrets/k/rollback", "[]", Admins),
        at("POST", "/admin/secrets/k/cap", "[]", Admins),
        at("DELETE", "/admin/secrets/k", "", Admins),
        at("GET", "/permissions", "", All),
        at("POST", "/permissions/reload", "", Editors),
        at("POST", "/summary/check", "", Editors),
        at("GET", "/admin/host", "", Admins),
        at("GET", "/admin/warnings", "", Admins),
        at("GET", "/admin/grants", "", Admins),
        at("POST", "/admin/storage", "[]", Admins),
        at("POST", "/admin/store/sweep", r#"{"dry_run":true}"#, Admins),
        at("POST", "/admin/folders/sweep", "", Admins),
        at("GET", "/admin/folders/browse", "", Admins),
        at("POST", "/admin/folders/pick", "{}", Admins),
        at("GET", "/admin/databases/none/tables", "", Admins),
        // People.
        at("GET", "/people", "", Admins),
        at("GET", "/admin/oidc", "", Admins),
        at("POST", "/people", "[]", Admins),
        at("PATCH", "/people/nobody", "{}", Admins),
        at("DELETE", "/people/nobody", "", Admins),
        at("POST", "/people/nobody/reset", "", Admins),
        at("POST", "/admin/oidc", "[]", Admins),
        // What is bound to whom, and its versions.
        at("GET", "/bindings", "", Admins),
        at("POST", "/bindings", "[]", Admins),
        at("DELETE", "/bindings/none", "", Admins),
        at("GET", "/access/versions", "", Admins),
        at("GET", "/access", "", Admins),
        // Roles and groups.
        at("GET", "/roles", "", Admins),
        at("PUT", "/roles/none", "[]", Admins),
        at("DELETE", "/roles/none", "", Admins),
        at("GET", "/groups", "", Admins),
        at("PUT", "/groups/none", "[]", Admins),
        at("DELETE", "/groups/none", "", Admins),
        // What the caller may do.
        at("GET", "/auth/me/can?actions=flow.view", "", SignedIn),
        // Paths no route has: a bare 404 to anyone signed in (the plan's D5).
        unrouted("GET", "/nope", All),
        unrouted("POST", "/nope", All),
        unrouted("POST", "/admin/nope", All),
        unrouted("DELETE", "/flows", All),
    ];
    for what in ["document", "versions", "events", "changes", "history", "manifest", "grant", "runs", "log", "sessions", "export", "replay", "state", "collections/c", "holds", "feeds", "queues"] {
        rows.push(at("GET", &format!("/deployments/d/{what}"), "", All));
    }
    for what in ["run", "pause", "stop"] {
        rows.push(at("POST", &format!("/deployments/none/{what}"), "", Editors));
    }
    for what in ["folders", "databases", "mails", "feeds", "queues"] {
        rows.push(at("GET", &format!("/admin/{what}"), "", Admins));
        rows.push(at("POST", &format!("/admin/{what}"), "[]", Admins));
        rows.push(at("DELETE", &format!("/admin/{what}/none"), "", Admins));
    }
    // ci is bob's, scout cy's.
    for (whose, owner) in [("ci", "bob"), ("scout", "cy")] {
        rows.push(at("GET", &format!("/people/{whose}/tokens"), "", Owners(owner)));
        rows.push(at("POST", &format!("/people/{whose}/tokens"), "[]", Owners(owner)));
        rows.push(at("DELETE", &format!("/people/{whose}/tokens/none"), "", Owners(owner)));
    }
    // Last: it ends a person's session.
    rows.push(at("POST", "/auth/sign-out", "", SignedIn));
    rows
}

/// `method` `path` sent to `state`, signed in with `token`: its status, and its body as text. Sent
/// as if through a proxy, so a folder dialog (pick.rs) is never shown on the machine running the
/// tests.
pub(super) fn ask(state: &AppState, token: Option<&str>, method: &str, path: &str, body: Vec<u8>) -> (u16, String) {
    let mut headers = vec![("X-Forwarded-For".to_string(), "203.0.113.9".to_string())];
    headers.extend(token.map(|t| ("Authorization".to_string(), format!("Bearer {t}"))));
    let answer = route(state, &req_with(method, path, body, headers));
    (answer.status_code, String::from_utf8_lossy(&body_bytes(answer)).into_owned())
}

/// The `error` an answer says, or nothing.
pub(super) fn error_of(said: &str) -> String {
    serde_json::from_str::<Value>(said).ok().and_then(|v| v["error"].as_str().map(String::from)).unwrap_or_default()
}

/// `slug` deployed as `unit`, with what its preview says needs approving approved.
pub(super) fn deploy(state: &AppState, token: Option<&str>, slug: &str, unit: &str, requirements: Value) -> (u16, String) {
    let asked = json!({ "slug": slug, "unit": unit, "requirements": requirements });
    let (code, preview) = ask(state, token, "POST", "/deploy/preview", serde_json::to_vec(&asked).unwrap());
    assert_eq!(code, 200, "{preview}");
    let needs = serde_json::from_str::<Value>(&preview).unwrap()["needs_approval"].clone();
    let body = json!({ "slug": slug, "unit": unit, "requirements": requirements, "acknowledge": needs });
    ask(state, token, "POST", "/deploy", serde_json::to_vec(&body).unwrap())
}

/// What a let-through request must leave as it was: the deployments (each one's status and
/// grant), the saved flows (each one's saver), the principals (what is bound to each, whether it
/// is disabled, its owner, and how many tokens it has), and the roles and groups.
fn kept(state: &AppState) -> String {
    let deployments: Vec<String> = state.deployments.lock().unwrap().iter().map(|(s, d)| format!("{s} {:?} {:?}", d.life.status, d.grant.items)).collect();
    let flows: Vec<String> = state.flows.lock().unwrap().values().map(|f| format!("{} {:?}", f.name, f.saved_by)).collect();
    let people: Vec<String> = state.access.accounts().map(|a| a.principals().iter().map(|p| format!("{} {:?} {} {:?} {}", p.name, a.bindings_of(&p.name), p.disabled, p.owner, a.tokens(&p.name).len())).collect()).unwrap_or_default();
    let defined = state.access.accounts().map(|a| format!("{:?} {:?}", a.access_document().roles, a.access_document().groups)).unwrap_or_default();
    format!("{deployments:?}\n{flows:?}\n{people:?}\n{defined}")
}

/// A server with the deployment `d` (the flow `echo`, deployed as its unit, answered here) and the
/// saved flow `f`; with accounts, every kind of caller.
fn server(accounts: bool) -> (AppState, String, Vec<Caller>) {
    let mut state = test_state();
    let unit = to_hex(&stage_text(&state, ECHO));
    assert_eq!(deploy(&state, None, "d", &unit, json!({})).0, 200);
    assert_eq!(ask(&state, None, "POST", "/flows", serde_json::to_vec(&json!({ "name": "f", "document": {} })).unwrap()).0, 200);
    let anyone = Caller { name: "no one", token: None, kind: None, role: None, disabled: false };
    if !accounts {
        return (state, unit, vec![anyone]);
    }
    let book = Arc::new(Accounts::memory());
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", PW).unwrap();
    let person = |name: &str, role: Role| book.accept(&book.invite(name, role.name(), None, "ada").unwrap(), PW).unwrap().0;
    let (cy, bob) = (person("cy", Role::Viewer), person("bob", Role::Editor));
    let local = book.new_local("bma");
    let made = |name: &str, kind, role: Role, owner: &str| {
        book.add(name, kind, role.name(), owner, "ada").unwrap();
        book.create_token(name, "the table", "ada").unwrap().0
    };
    let claude = made("claude", Kind::Agent, Role::Agent, "ada");
    let scout = made("scout", Kind::Agent, Role::Viewer, "cy");
    let ci = made("ci", Kind::Service, Role::Editor, "bob");
    let nightly = made("nightly", Kind::Service, Role::Viewer, "bob");
    let off = made("off", Kind::Service, Role::Editor, "bob");
    book.set_disabled("off", true).unwrap();
    state.access = access::Access::new(book, vec![], vec![]);
    let caller = |name, token: String, kind, role| Caller { name, token: Some(token), kind: Some(kind), role: Some(role), disabled: false };
    let callers = vec![
        anyone,
        caller("cy", cy, Kind::Person, Role::Viewer),
        caller("bob", bob, Kind::Person, Role::Editor),
        caller("ada", ada, Kind::Person, Role::Admin),
        caller("local:bma", local, Kind::Person, Role::Admin),
        caller("claude", claude, Kind::Agent, Role::Agent),
        caller("scout", scout, Kind::Agent, Role::Viewer),
        caller("ci", ci, Kind::Service, Role::Editor),
        caller("nightly", nightly, Kind::Service, Role::Viewer),
        Caller { disabled: true, ..caller("off", off, Kind::Service, Role::Editor) },
    ];
    (state, unit, callers)
}

/// Every route, sent by every kind of caller (no one; a person who is a viewer, an editor or an
/// admin; the run file's admin; an agent holding the agent role, and one holding the viewer role;
/// a service that is an editor, one that is a viewer, and one disabled), is let through or refused
/// as the table says, with the refusal's words; and on a server without accounts everyone is let
/// through but to decide an approval. A route someone is let through reaches its router (the
/// admin isn't answered a bare 404), and nothing a request let through does changes the server.
#[test]
fn every_route_lets_through_whom_its_table_says_and_refuses_the_rest_in_its_words() {
    let (state, unit, callers) = server(true);
    let before = kept(&state);
    let mut wrong = vec![];
    for row in &rows(&unit) {
        for c in &callers {
            let (code, said) = ask(&state, c.token.as_deref(), row.method, &row.path, row.body.as_bytes().to_vec());
            let asked = format!("{} {} by {} ({:?})", row.method, row.path, c.name, row.today);
            match expected(row.today, c) {
                Some((status, words)) if (code, error_of(&said)) != (status, words.clone()) => wrong.push(format!("{asked}: refused {status} {words:?}, answered {code} {said}")),
                Some(_) => {}
                None if code == 401 || (code == 403 && error_of(&said).starts_with(&format!("{} may not do that:", c.name))) => wrong.push(format!("{asked}: let through, answered {code} {said}")),
                None if row.routed && c.name == "ada" && code == 404 && said.is_empty() => wrong.push(format!("{asked}: no router answers it")),
                None => {}
            }
        }
    }
    assert!(wrong.is_empty(), "{} answers aren't the table's:\n{}", wrong.len(), wrong.join("\n"));
    assert_eq!(kept(&state), before, "a request let through changed nothing");

    let (open, unit, anyone) = server(false);
    let before = kept(&open);
    for row in &rows(&unit) {
        let (code, said) = ask(&open, None, row.method, &row.path, row.body.as_bytes().to_vec());
        let asked = format!("{} {} on a server without accounts", row.method, row.path);
        if row.today == Today::Deciders {
            assert_eq!((code, error_of(&said)), (403, NO_ACCOUNTS.to_string()), "{asked}");
        } else {
            assert!(code != 401 && !(code == 403 && error_of(&said).contains("may not do that")), "{asked} is let through: answered {code} {said}");
        }
    }
    assert_eq!(anyone.len(), 1);
    assert_eq!(kept(&open), before, "a request let through changed nothing");
}

/// What isn't a role's, for each kind of caller:
/// - an agent saves over only a flow it saved, an editor over anyone's;
/// - an agent runs an action only once the flow's grant lets agents (`AgentRuns`), and reads its
///   log lines only once it lets them (`AgentReadsLogs`), whatever role it holds (one holding the
///   viewer role reads no more than one holding the agent role);
/// - an agent never approves what a flow may do: a deploy of its that needs approval is a
///   proposal, which a person's deploy of that version approves; it is refused where a person
///   turned proposing off, and never starts a stopped deployment, which a person does;
/// - an agent's restore is a dry run, or saved as an ask.
#[test]
fn an_agent_is_held_to_its_own_flows_and_to_what_each_flow_lets_agents() {
    let (state, unit, callers) = server(true);
    let token = |name: &str| callers.iter().find(|c| c.name == name).unwrap().token.clone().unwrap();
    let (ada, bob, claude) = (token("ada"), token("bob"), token("claude"));
    let send = |t: &str, method: &str, path: &str, body: Value| ask(&state, Some(t), method, path, if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() });
    let refused = |(code, said): (u16, String), words: &str| code == 403 && error_of(&said).contains(words);

    // Its own flows.
    let save = |t: &str, name: &str| send(t, "POST", "/flows", json!({ "name": name, "document": {} }));
    assert_eq!(save(&ada, "adas").0, 200);
    let someone_elses = "an agent doesn't replace someone else's flow";
    assert!(refused(save(&claude, "adas"), someone_elses), "saved by ada");
    assert_eq!(save(&claude, "claudes").0, 200, "a new one");
    assert_eq!(save(&claude, "claudes").0, 200, "its own");
    assert_eq!(save(&bob, "claudes").0, 200, "an editor saves over anyone's");
    assert!(refused(save(&claude, "claudes"), someone_elses), "bob's now");

    // What the flow lets agents do, before and after a person lets them.
    let lets = |granted: bool| {
        for c in callers.iter().filter(|c| c.token.is_some() && !c.disabled) {
            let t = c.token.as_deref().unwrap();
            let (ran, read) = (send(t, "POST", "/trigger/d/echo", json!({})), send(t, "GET", "/deployments/d/logs", Value::Null));
            let asked = format!("{} (granted: {granted})", c.name);
            match c.role.unwrap() {
                Role::Viewer => assert_eq!((ran.0, error_of(&ran.1)), (403, format!("{} may not do that: {VIEWERS}", c.name)), "{asked}"),
                Role::Agent if !granted => assert!(refused(ran.clone(), "agents may not run \"d\"'s action \"echo\""), "{asked}: {ran:?}"),
                _ => assert_eq!(ran.0, 200, "{asked}: {ran:?}"),
            }
            if c.kind == Some(Kind::Agent) && !granted {
                assert!(refused(read.clone(), "agents may not read \"d\"'s log lines"), "{asked}: {read:?}");
            } else {
                assert_eq!(read.0, 200, "{asked}: {read:?}");
            }
        }
    };
    lets(false);
    assert_eq!(deploy(&state, Some(&ada), "d", &unit, json!({ "agents": ["echo"], "agent_logs": true })).0, 200);
    lets(true);

    // Deploying: an agent proposes, a person approves.
    let deploy_as = |t: &str, body: Value| send(t, "POST", "/deploy", body);
    let approves = "an agent doesn't approve what a flow may do";
    assert!(refused(deploy_as(&claude, json!({ "slug": "d", "unit": unit, "acknowledge": ["Action(\"echo\")"] })), approves));
    let (code, said) = deploy_as(&claude, json!({ "slug": "e", "unit": unit, "why": "an echo", "document": {} }));
    assert_eq!(code, 202, "a proposal: {said}");
    assert_eq!(deploy(&state, Some(&ada), "e", &unit, json!({})).0, 200);
    let proposed = state.proposals.all().into_iter().find(|p| p.slug == "e").unwrap();
    assert_eq!(proposed.state, crate::proposals::Decision::Approved, "a person's deploy of it approved it");
    assert_eq!(send(&ada, "POST", "/deployments/e/settings", json!({ "agents_propose": false })).0, 200);
    assert!(refused(deploy_as(&claude, json!({ "slug": "e", "unit": unit })), "a person turned off agents proposing versions of \"e\""));
    assert_eq!(send(&ada, "POST", "/deployments/e/settings", json!({ "agents_propose": true })).0, 200);
    assert_eq!(send(&ada, "POST", "/deployments/e/stop", Value::Null).0, 200);
    assert!(refused(deploy_as(&claude, json!({ "slug": "e", "unit": unit })), "deployment \"e\" is stopped, and an agent doesn't start one"));
    assert_eq!(deploy_as(&bob, json!({ "slug": "e", "unit": unit })).0, 200, "a person starts it");

    // Restoring: an agent's is a dry run, or an ask.
    let restore = |t: &str, dry_run: bool| send(t, "POST", "/deployments/d/restore", json!({ "version": 1, "code": true, "dry_run": dry_run }));
    assert_eq!(restore(&claude, true).0, 200, "an agent's dry run");
    assert_eq!(restore(&claude, false).0, 202, "an agent's restore is saved as an ask");
    assert_eq!(restore(&bob, true).0, 200, "a person's dry run");
}
