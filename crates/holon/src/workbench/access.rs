//! The admin page's access parts, as the web console's People pages: the groups, the roles, what
//! is bound to whom, the access document's history and its approval settings, and your own agents
//! and services. Each says what the command line says (accesscmd.rs), and takes what its dialog
//! line says as `holon groups`, `holon roles`, `holon bind` and `holon access approvals` take
//! their flags. A change that gives more, on a host where access changes wait for people, waits:
//! the line says so, and Approvals › Access changes decides it.

use std::collections::BTreeMap;

use ratatui::crossterm::event::KeyCode;
use serde_json::{json, Value};

use super::admin::{AdminAsk, Part};
use super::app::{App, Tone};
use super::flow::Row;
use crate::accesscmd::{self, subject};
use crate::mcp::seg;

/// What a dialog of the access parts asks for.
pub(crate) enum AccessAsk {
    /// A new group: its name, its members, `--about`, `--why`.
    NewGroup,
    /// Members added to a group, or taken out of it.
    Members { group: String, add: bool },
    /// A group removed: why.
    RemoveGroup(String),
    /// A role defined, or one changed (its name): what it allows, `--kinds`, `--about`, `--why`.
    Role,
    /// A role removed: why.
    RemoveRole(String),
    /// A role bound, as `holon bind` takes it.
    Bind,
    /// A binding taken away: why.
    Unbind(u64),
    /// The approval settings, as `holon access approvals` takes them.
    Approvals,
    /// An API key for your agent or service: what it is for.
    Token(String),
    /// One of its keys revoked: which.
    Revoke(String),
}

/// A dialog's line read as a command's words and flags. `--why` and `--about` take every word up
/// to the next flag; the others one word each. A flag not in `valued` is refused.
pub(crate) fn read_flags(text: &str, valued: &[&str]) -> Result<(Vec<String>, BTreeMap<String, String>), String> {
    let (mut words, mut flags) = (vec![], BTreeMap::new());
    let mut it = text.split_whitespace().peekable();
    while let Some(w) = it.next() {
        if !w.starts_with("--") {
            words.push(w.to_string());
            continue;
        }
        if !valued.contains(&w) {
            return Err(format!("{w} isn't one of {}", valued.join(", ")));
        }
        let value = if matches!(w, "--why" | "--about") {
            let mut said = vec![];
            while let Some(n) = it.peek().copied().filter(|n| !n.starts_with("--")) {
                said.push(n);
                it.next();
            }
            said.join(" ")
        } else {
            it.next().filter(|v| !v.starts_with("--")).ok_or_else(|| format!("{w} takes a value"))?.to_string()
        };
        flags.insert(w.trim_start_matches("--").to_string(), value);
    }
    Ok((words, flags))
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("-").to_string()
}

fn words(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect()
}

fn field(k: &str, v: impl Into<String>) -> Row {
    Row::Field(k.to_string(), v.into(), Tone::Plain)
}

fn text(t: Tone, s: impl Into<String>) -> Row {
    Row::Text(t, s.into())
}

/// `?why=…`, when one is given.
fn why_query(why: &str) -> String {
    if why.trim().is_empty() { String::new() } else { format!("?why={}", seg(why.trim())) }
}

/// An access change's answer said: what waits for people, and where it is decided; else `done`.
fn waits_or(v: &Value, done: impl FnOnce(&Value) -> String) -> String {
    match accesscmd::waiting(v) {
        Some(line) => format!("{line}: Approvals › Access changes decides it"),
        None => done(v),
    }
}

/// The settings as `holon access approvals` takes them, from what `GET /access` says.
fn approvals_line(a: &Value) -> String {
    let on = |v: &Value, default: bool| if v.as_bool().unwrap_or(default) { "on" } else { "off" };
    let changes = a["changes"].as_u64().map_or("off".to_string(), |n| n.to_string());
    format!("--changes {changes} --exclude-deployer {} --agents {} --starter {}", on(&a["exclude_deployer"], false), on(&a["agents"], true), on(&a["starter"], true))
}

/// One version of the access document whole, as `holon access show --version N` says it, a row
/// a thing, so a narrow pane wraps each on its own: who made it and why, the approval settings,
/// then its roles (an admin's), groups and bindings.
fn version_rows(v: &Value, now: u64) -> Vec<Row> {
    let mut rows = vec![text(Tone::Head, format!("Version {}", v["version"])), field("What", s(&v["what"])), field("By", format!("{}, {}", s(&v["by"]), crate::cmdline::when(v["at"].as_u64())))];
    if let Some(why) = v["why"].as_str() {
        rows.push(field("Why", why));
    }
    rows.push(field("Run file", v["local"].as_str().map_or("tied to no account".into(), |name| format!("signs in as {name}"))));
    let settings: crate::bindings::ApprovalSettings = serde_json::from_value(v["approvals"].clone()).unwrap_or_default();
    rows.extend(settings.each().map(|(name, value, said)| Row::Field(name.replace('_', "-"), format!("{value}: {said}"), Tone::Plain)));
    let list = |k: &str| v[k].as_array().cloned().unwrap_or_default();
    let (roles, groups, bindings) = (list("roles"), list("groups"), list("bindings"));
    rows.push(text(Tone::Head, format!("Roles defined ({})", roles.len())));
    rows.extend(roles.iter().map(|r| text(Tone::Plain, format!("  {} ({}): {}", s(&r["name"]), words(&r["kinds"]).join(", "), words(&r["allows"]).join(", ")))));
    rows.push(text(Tone::Head, format!("Groups ({})", groups.len())));
    rows.extend(groups.iter().map(|g| text(Tone::Plain, format!("  {}: {}", s(&g["name"]), words(&g["members"]).join(", ")))));
    rows.push(text(Tone::Head, format!("Bindings ({})", bindings.len())));
    rows.extend(bindings.iter().map(|b| {
        let until = match b["expires_at"].as_u64() {
            Some(at) if at <= now => format!(", until {} (expired)", crate::cmdline::when(Some(at))),
            Some(at) => format!(", until {}", crate::cmdline::when(Some(at))),
            None => String::new(),
        };
        text(Tone::Plain, format!("  #{} {} to {} {}{until}, by {}", b["id"], s(&b["role"]), subject(&b["who"]), accesscmd::place(&b["scope"]), s(&b["by"])))
    }));
    rows
}

/// What a key of an access part takes, as can.rs's routes ask it (none for your own agents: the
/// server asks people.manage on the principal, which its owner holds).
pub(crate) fn takes(p: Part, k: KeyCode, row: usize) -> &'static [&'static str] {
    match (p, k) {
        (Part::Groups, KeyCode::Char('n' | '+' | '-' | 'D') | KeyCode::Delete) => &["people.assign"],
        (Part::Roles, KeyCode::Char('n' | 'D') | KeyCode::Enter | KeyCode::Delete) => &["role.define"],
        (Part::Bindings, KeyCode::Char('n' | 'D') | KeyCode::Delete) => &["people.assign"],
        (Part::History, KeyCode::Enter) if row == 0 => &["people.assign"],
        _ => &[],
    }
}

impl App {
    /// How many rows an access part has.
    pub(crate) fn access_part_rows(&self, p: Part) -> usize {
        let a = &self.admin;
        match p {
            Part::Groups => a.groups.len(),
            Part::Roles => a.roles.len(),
            Part::Bindings => a.bindings.len(),
            Part::History => 1 + a.versions.len(),
            Part::Yours => a.owned.len(),
            _ => 0,
        }
    }

    /// Read the access parts' lists: each only by whoever may see it, else why not.
    pub(crate) fn access_part_load(&mut self) {
        let Some(c) = self.client.as_ref() else { return };
        let read = |path: &str, action: Option<&str>| match action.map(|a| c.may(a, None)).unwrap_or(Ok(())) {
            Ok(()) => c.get(path),
            Err(why) => Err(why),
        };
        let groups = read("/groups", Some("people.view"));
        let bindings = read("/bindings", Some("people.view"));
        let versions = read("/access/versions", Some("people.view"));
        let access = read("/access", Some("people.view"));
        let owned = read("/auth/owned", None);
        let roles = read("/roles", Some("role.view"));
        let a = &mut self.admin;
        a.refused[Part::Groups as usize] = groups.as_ref().err().cloned();
        a.refused[Part::Roles as usize] = roles.as_ref().err().cloned();
        a.refused[Part::Bindings as usize] = bindings.as_ref().err().cloned();
        a.refused[Part::History as usize] = versions.as_ref().err().cloned();
        a.refused[Part::Yours as usize] = owned.as_ref().err().cloned();
        // The roles a principal may be given, for the role dialogs too: none read, they say "-".
        a.roles_version = roles.as_ref().map(|r| r["version"].clone()).unwrap_or(Value::Null);
        a.roles = roles.map(|r| r["roles"].as_array().cloned().unwrap_or_default()).unwrap_or_default();
        a.groups_version = groups.as_ref().map(|g| g["version"].clone()).unwrap_or(Value::Null);
        a.groups = groups.map(|g| g["groups"].as_array().cloned().unwrap_or_default()).unwrap_or_default();
        a.bindings = bindings.map(|b| b["bindings"].as_array().cloned().unwrap_or_default()).unwrap_or_default();
        a.versions = versions.map(|v| v["versions"].as_array().cloned().unwrap_or_default()).unwrap_or_default();
        a.access = access.unwrap_or(Value::Null);
        a.owned = owned.map(|o| o.as_array().cloned().unwrap_or_default()).unwrap_or_default();
    }

    /// The right pane for an access part's row `i`.
    pub(crate) fn access_part_pane(&self, p: Part, i: usize) -> Vec<Row> {
        let a = &self.admin;
        let Some(c) = self.client.as_ref() else { return vec![] };
        let bound_to = |group: &str| -> Vec<Row> {
            a.bindings.iter().filter(|b| b["who"]["group"] == group).map(|b| text(Tone::Plain, format!("  #{} {} {}", b["id"], s(&b["role"]), accesscmd::place(&b["scope"])))).collect()
        };
        match p {
            Part::Groups => match a.groups.get(i) {
                Some(g) => {
                    let name = s(&g["name"]);
                    let members = words(&g["members"]);
                    let mut rows = vec![text(Tone::Head, name.clone())];
                    if let Some(about) = g["about"].as_str() {
                        rows.push(field("About", about));
                    }
                    rows.push(field("Members", if members.is_empty() { "no one".into() } else { members.join(", ") }));
                    let bound = bound_to(&name);
                    rows.push(field("Bound to it", if bound.is_empty() { "nothing: what its members hold is their own".to_string() } else { format!("{} binding{}", bound.len(), if bound.len() == 1 { "" } else { "s" }) }));
                    rows.extend(bound);
                    rows.push(text(Tone::Dim, "+ adds members · - takes some out · del removes it, once nothing is bound to it · n makes another"));
                    rows
                }
                None => vec![text(Tone::Dim, "No groups yet. n makes one: its name and its members; a role bound to it holds for each of them.")],
            },
            Part::Roles => match a.roles.get(i) {
                Some(r) => {
                    let builtin = r["builtin"] == true;
                    let mut rows = vec![text(Tone::Head, format!("{}{}", s(&r["name"]), if builtin { "  (built in)" } else { "" }))];
                    if let Some(about) = r["about"].as_str() {
                        rows.push(field("About", about));
                    }
                    rows.push(field("Held by", words(&r["kinds"]).join(" or ")));
                    rows.push(field("Bound", format!("{} time{}", r["bound"], if r["bound"] == 1 { "" } else { "s" })));
                    let allows = words(&r["allows"]);
                    rows.push(text(Tone::Head, format!("Allows {} action{}", allows.len(), if allows.len() == 1 { "" } else { "s" })));
                    rows.extend(allows.into_iter().map(|a| text(Tone::Plain, format!("  {a}"))));
                    rows.push(text(Tone::Dim, if builtin { "Built in: it stays as it is. n defines a role of your own." } else { "enter changes it · del removes it, once nothing is bound to it · n defines another" }));
                    rows
                }
                None => vec![text(Tone::Dim, "n defines a role: its name and the actions it allows.")],
            },
            Part::Bindings => match a.bindings.get(i) {
                Some(b) => {
                    let until = match b["expires_at"].as_u64() {
                        Some(at) => format!("{}{}", crate::cmdline::when(Some(at)), if b["expired"] == true { " (expired: it no longer holds)" } else { "" }),
                        None => "for good".into(),
                    };
                    vec![
                        text(Tone::Head, format!("Binding {}: {}", b["id"], s(&b["role"]))),
                        field("To", subject(&b["who"])),
                        field("Where", accesscmd::place(&b["scope"])),
                        field("Until", until),
                        field("By", s(&b["by"])),
                        field("At", crate::cmdline::when(b["at"].as_u64())),
                        text(Tone::Dim, "del unbinds it, saying why · n binds a role"),
                    ]
                }
                None => vec![text(Tone::Dim, "Nothing is bound yet. n binds a role to someone or a group, on the host, a flow, flows with a tag, or one approval point.")],
            },
            Part::History if i == 0 => {
                let mut rows = vec![text(Tone::Head, "Who approves what, on this host")];
                let settings: crate::bindings::ApprovalSettings = serde_json::from_value(a.access["approvals"].clone()).unwrap_or_default();
                rows.extend(settings.each().map(|(name, value, said)| Row::Field(name.replace('_', "-"), format!("{value}: {said}"), Tone::Plain)));
                rows.push(field("Version", a.access["version"].to_string()));
                rows.push(text(Tone::Dim, "enter changes them, as holon access approvals takes them"));
                rows
            }
            Part::History => {
                let Some(v) = a.versions.get(i - 1) else { return vec![] };
                let n = v["version"].as_u64().unwrap_or(0);
                match c.get(&format!("/access?version={n}")) {
                    Ok(doc) => version_rows(&doc, crate::deploy::epoch_now()),
                    Err(e) => vec![text(Tone::Head, format!("Version {n}")), text(Tone::Bad, e)],
                }
            }
            Part::Yours => match a.owned.get(i) {
                Some(o) => {
                    let name = s(&o["name"]);
                    let mut rows = vec![
                        text(Tone::Head, name.clone()),
                        field("Kind", if o["kind"] == "agent" { "an agent" } else { "a service" }),
                        field("Holds", accesscmd::bindings_said(&o["bindings"])),
                        Row::Field("State".into(), if o["disabled"] == true { "disabled: its keys are refused".into() } else { "active".into() }, if o["disabled"] == true { Tone::Warn } else { Tone::Plain }),
                    ];
                    let tokens = o["tokens"].as_array().cloned().unwrap_or_default();
                    rows.push(text(Tone::Head, format!("API keys ({})", tokens.len())));
                    rows.extend(tokens.iter().map(|t| text(Tone::Plain, format!("  {}  {}  made {}", s(&t["label"]), s(&t["id"]), crate::mcp::when(&t["created_at"])))));
                    rows.push(field("Connect it", format!("holon mcp setup claude-code --server {}", c.server())));
                    rows.push(text(Tone::Dim, "n makes an API key for it · del revokes one"));
                    rows
                }
                None => vec![text(Tone::Dim, "No agent or service of yours. An admin adds one, owned by you; then you make its keys here.")],
            },
            _ => vec![],
        }
    }

    /// A key on an access part, its takes already allowed.
    pub(crate) fn access_part_key(&mut self, p: Part, k: KeyCode, i: usize) {
        let a = &self.admin;
        match (p, k) {
            (Part::Groups, KeyCode::Char('n')) => self.ask(
                "Make a group",
                &["Its name and its members, then any of --about TEXT --why TEXT:", "payments ana bob --about the payments team", "A role bound to the group holds for each of its members."],
                "",
                AdminAsk::Access(AccessAsk::NewGroup),
            ),
            (Part::Groups, KeyCode::Char(c @ ('+' | '-'))) => {
                if let Some(g) = a.groups.get(i) {
                    let group = s(&g["name"]);
                    let add = c == '+';
                    let lines = [if add { "Who joins it, and --why TEXT if you say why:" } else { "Who leaves it, and --why TEXT if you say why:" }, "ana bob"];
                    self.ask(format!("{} {group}", if add { "Add to" } else { "Take out of" }), &lines, "", AdminAsk::Access(AccessAsk::Members { group, add }));
                }
            }
            (Part::Groups, KeyCode::Delete | KeyCode::Char('D')) => {
                if let Some(g) = a.groups.get(i) {
                    let group = s(&g["name"]);
                    self.ask(format!("Remove the group {group}?"), &["Why (it is kept with the access version):", "It can't be removed while a role is bound to it: unbind that first."], "", AdminAsk::Access(AccessAsk::RemoveGroup(group)));
                }
            }
            (Part::Roles, KeyCode::Char('n')) => {
                let catalogue = format!("The actions: {}.", crate::can::Action::ALL.map(crate::can::Action::name).join(", "));
                self.ask(
                    "Define a role",
                    &["Its name and the actions it allows, then any of --kinds person,agent,service --about TEXT --why TEXT:", "payout-approver run.view,run.decide --kinds person", catalogue.as_str()],
                    "",
                    AdminAsk::Access(AccessAsk::Role),
                );
            }
            (Part::Roles, KeyCode::Enter) => {
                let Some(r) = a.roles.get(i).filter(|r| r["builtin"] != true) else { return self.fail("a built-in role stays as it is: n defines one of your own") };
                let about = r["about"].as_str().map(|a| format!(" --about {a}")).unwrap_or_default();
                let now = format!("{} {} --kinds {}{about}", s(&r["name"]), words(&r["allows"]).join(","), words(&r["kinds"]).join(","));
                let catalogue = format!("The actions: {}.", crate::can::Action::ALL.map(crate::can::Action::name).join(", "));
                self.ask(format!("Change the role {}", s(&r["name"])), &["What it allows and who may hold it, and --why TEXT:", catalogue.as_str()], now, AdminAsk::Access(AccessAsk::Role));
            }
            (Part::Roles, KeyCode::Delete | KeyCode::Char('D')) => {
                let Some(r) = a.roles.get(i).filter(|r| r["builtin"] != true) else { return self.fail("a built-in role stays as it is") };
                let role = s(&r["name"]);
                self.ask(format!("Remove the role {role}?"), &["Why (it is kept with the access version):", "It can't be removed while it is bound: unbind it first."], "", AdminAsk::Access(AccessAsk::RemoveRole(role)));
            }
            (Part::Bindings, KeyCode::Char('n')) => self.ask_bind(""),
            (Part::Bindings, KeyCode::Delete | KeyCode::Char('D')) => {
                if let Some(b) = a.bindings.get(i) {
                    let id = b["id"].as_u64().unwrap_or(0);
                    let title = format!("Unbind {} from {} {}?", s(&b["role"]), subject(&b["who"]), accesscmd::place(&b["scope"]));
                    self.ask(title, &["Why (it is kept with the access version):"], "", AdminAsk::Access(AccessAsk::Unbind(id)));
                }
            }
            (Part::History, KeyCode::Enter) if i == 0 => {
                let now = approvals_line(&a.access["approvals"]);
                let lines = [
                    "As holon access approvals takes them, and --why TEXT:",
                    "--changes N: how many people approve an access change that gives more (1 to 5), or off",
                    "--exclude-deployer, --agents, --starter: on or off",
                ];
                self.ask("Approval settings", &lines, now, AdminAsk::Access(AccessAsk::Approvals));
            }
            (Part::Yours, KeyCode::Char('n')) => {
                if let Some(o) = a.owned.get(i) {
                    let name = s(&o["name"]);
                    self.ask(format!("An API key for {name}"), &["What it is for:", "ci nightly report", "The key is shown once, and copied to your clipboard."], "", AdminAsk::Access(AccessAsk::Token(name)));
                }
            }
            (Part::Yours, KeyCode::Delete | KeyCode::Char('D')) => {
                if let Some(o) = a.owned.get(i) {
                    let name = s(&o["name"]);
                    let newest = o["tokens"].as_array().and_then(|t| t.last()).map(|t| s(&t["id"])).unwrap_or_default();
                    self.ask(format!("Revoke one of {name}'s API keys"), &["Which, by its id (its detail lists them): what signs in with it is refused from now on."], newest, AdminAsk::Access(AccessAsk::Revoke(name)));
                }
            }
            _ => {}
        }
    }

    /// The bind dialog, as `holon bind` takes it, `with` already in it (`--to ana`).
    pub(crate) fn ask_bind(&mut self, with: &str) {
        self.ask(
            "Bind a role",
            &[
                "A role and to whom (--to NAME or --group GROUP), then where (none: the host) and any of",
                "--flow SLUG  --tag TAG  --approval FLOW:POINT  --until 7d|2026-12-01  --why TEXT",
                "payout-approver --to ana --approval payout:ok --until 30d --why covering for bob",
            ],
            with,
            AdminAsk::Access(AccessAsk::Bind),
        );
    }

    /// A dialog of an access part answered.
    pub(crate) fn access_part_answer(&mut self, ask: AccessAsk, value: String) {
        let Some(c) = self.client.as_ref() else { return };
        match ask {
            AccessAsk::NewGroup | AccessAsk::Members { .. } => {
                let (names, flags) = match read_flags(&value, &["--about", "--why"]) {
                    Ok(r) => r,
                    Err(e) => return self.fail(e),
                };
                let (group, members, about) = match &ask {
                    AccessAsk::Members { group, add } => {
                        let now = self.admin.groups.iter().find(|g| g["name"] == group.as_str());
                        let mut members = now.map(|g| words(&g["members"])).unwrap_or_default();
                        if names.is_empty() {
                            return self.fail("whom: their names");
                        }
                        if *add {
                            members.extend(names.iter().filter(|n| !members.contains(n)).cloned().collect::<Vec<_>>());
                        } else {
                            members.retain(|m| !names.contains(m));
                        }
                        (group.clone(), members, flags.get("about").cloned().or_else(|| now.and_then(|g| g["about"].as_str().map(String::from))))
                    }
                    _ => {
                        let [group, members @ ..] = names.as_slice() else { return self.fail("its name, then its members: payments ana bob") };
                        if self.admin.groups.iter().any(|g| g["name"] == group.as_str()) {
                            return self.fail(format!("there is a group {group} already: + adds members to it"));
                        }
                        (group.clone(), members.to_vec(), flags.get("about").cloned())
                    }
                };
                // Made from the groups as read here (`base`): a change made since is refused, not undone.
                let body = json!({ "members": members, "about": about, "why": flags.get("why"), "base": self.admin.groups_version });
                let r = c.put(&format!("/groups/{}", seg(&group)), body);
                self.admin_done(r, |v| {
                    waits_or(v, |v| {
                        let members = words(&v["group"]["members"]);
                        format!("{group}: {} (access version {})", if members.is_empty() { "no one".into() } else { members.join(", ") }, v["version"])
                    })
                });
            }
            AccessAsk::RemoveGroup(group) => {
                let r = c.delete(&format!("/groups/{}{}", seg(&group), why_query(&value)));
                self.admin_done(r, |_| format!("removed the group {group}"));
            }
            AccessAsk::Role => {
                let (said, flags) = match read_flags(&value, &["--kinds", "--about", "--why"]) {
                    Ok(r) => r,
                    Err(e) => return self.fail(e),
                };
                let [name, allows @ ..] = said.as_slice() else { return self.fail("its name, then what it allows: payout-approver run.view,run.decide") };
                let allows: Vec<String> = allows.join(",").split(',').map(str::trim).filter(|a| !a.is_empty()).map(String::from).collect();
                if allows.is_empty() {
                    return self.fail("and the actions it allows, as the catalogue names them: run.view,run.decide");
                }
                if let Some(a) = allows.iter().find(|a| crate::can::Action::named(a).is_none()) {
                    return self.fail(format!("{a:?} isn't an action: the catalogue names {}", crate::can::Action::ALL.map(crate::can::Action::name).join(", ")));
                }
                let kinds: Vec<String> = flags.get("kinds").map_or("person", String::as_str).split(',').map(str::trim).filter(|k| !k.is_empty()).map(String::from).collect();
                if let Some(k) = kinds.iter().find(|k| !["person", "agent", "service"].contains(&k.as_str())) {
                    return self.fail(format!("{k:?} isn't a kind: person, agent or service"));
                }
                let name = name.clone();
                let r = c.put(&format!("/roles/{}", seg(&name)), json!({ "allows": allows, "kinds": kinds, "about": flags.get("about"), "why": flags.get("why"), "base": self.admin.roles_version }));
                self.admin_done(r, |v| waits_or(v, |v| format!("{name}: allows {}, held by {} (access version {})", words(&v["role"]["allows"]).join(", "), words(&v["role"]["kinds"]).join(" or "), v["version"])));
            }
            AccessAsk::RemoveRole(role) => {
                let r = c.delete(&format!("/roles/{}{}", seg(&role), why_query(&value)));
                self.admin_done(r, |_| format!("removed the role {role}"));
            }
            AccessAsk::Bind => {
                let (said, flags) = match read_flags(&value, &["--to", "--group", "--flow", "--tag", "--approval", "--until", "--why"]) {
                    Ok(r) => r,
                    Err(e) => return self.fail(e),
                };
                let [role] = said.as_slice() else { return self.fail("one role, and to whom: payout-approver --to ana") };
                let who = match (flags.get("to"), flags.get("group")) {
                    (Some(name), None) => json!({ "principal": name }),
                    (None, Some(group)) => json!({ "group": group }),
                    _ => return self.fail("to whom: --to NAME or --group GROUP, one of them"),
                };
                let scope = match accesscmd::scope(flags.get("flow").map(String::as_str), flags.get("tag").map(String::as_str), flags.get("approval").map(String::as_str)) {
                    Ok(s) => s,
                    Err(e) => return self.fail(e),
                };
                let expires_at = match flags.get("until").map(|u| accesscmd::until(u, crate::deploy::epoch_now())).transpose() {
                    Ok(at) => at,
                    Err(e) => return self.fail(e),
                };
                let role = role.clone();
                let r = c.post("/bindings", json!({ "who": who, "role": role, "scope": scope, "expires_at": expires_at, "why": flags.get("why") }));
                self.admin_done(r, |v| {
                    waits_or(v, |v| {
                        let b = &v["binding"];
                        let until = b["expires_at"].as_u64().map(|at| format!(", until {}", crate::cmdline::when(Some(at)))).unwrap_or_default();
                        format!("bound {role} to {} {}{until} (binding {}, access version {})", subject(&b["who"]), accesscmd::place(&b["scope"]), b["id"], v["version"])
                    })
                });
            }
            AccessAsk::Unbind(id) => {
                let r = c.delete(&format!("/bindings/{id}{}", why_query(&value)));
                self.admin_done(r, |_| format!("unbound binding {id}"));
            }
            AccessAsk::Approvals => {
                let (said, flags) = match read_flags(&value, &["--changes", "--exclude-deployer", "--agents", "--starter", "--why"]) {
                    Ok(r) => r,
                    Err(e) => return self.fail(e),
                };
                if !said.is_empty() {
                    return self.fail(format!("{} isn't a setting: each is a flag, --changes 2", said.join(" ")));
                }
                let mut change = serde_json::Map::new();
                for (flag, key) in [("exclude-deployer", "exclude_deployer"), ("agents", "agents"), ("starter", "starter")] {
                    match flags.get(flag).map(String::as_str) {
                        None => {}
                        Some(on @ ("on" | "off")) => {
                            change.insert(key.into(), (on == "on").into());
                        }
                        Some(other) => return self.fail(format!("--{flag} is on or off, not {other:?}")),
                    }
                }
                match flags.get("changes").map(String::as_str) {
                    None => {}
                    Some("off") => {
                        change.insert("changes".into(), Value::Null);
                    }
                    Some(n) => match n.parse::<u8>() {
                        Ok(n @ 1..=5) => {
                            change.insert("changes".into(), n.into());
                        }
                        _ => return self.fail(format!("--changes is how many people approve an access change, 1 to 5, or off: not {n:?}")),
                    },
                }
                if change.is_empty() {
                    return self.fail("nothing to change: give --changes, --exclude-deployer, --agents or --starter");
                }
                change.insert("why".into(), flags.get("why").cloned().into());
                let r = c.put("/access/approvals", Value::Object(change));
                self.admin_done(r, |v| waits_or(v, |v| format!("approval settings changed: {} (access version {})", approvals_line(&v["approvals"]), v["version"])));
            }
            AccessAsk::Token(name) => {
                let label = if value.trim().is_empty() { "made in holon's workbench".to_string() } else { value.trim().to_string() };
                match c.post(&format!("/people/{}/tokens", seg(&name)), json!({ "label": label })) {
                    Ok(v) => {
                        let token = v["token"].as_str().unwrap_or_default().to_string();
                        self.admin_load();
                        self.show_once(format!("{name}: an API key"), "The key", token, "It is shown only now: keep it where it reads it (HOLON_TOKEN).");
                    }
                    Err(e) => self.fail(e),
                }
            }
            AccessAsk::Revoke(name) => {
                let id = value.trim().to_string();
                if id.is_empty() {
                    return self.fail("which key: its id, as its detail lists it");
                }
                let r = c.delete(&format!("/people/{}/tokens/{}", seg(&name), seg(&id)));
                self.admin_done(r, |_| format!("revoked {name}'s {id}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dialog_line_reads_as_its_command_takes_it() {
        let (words, flags) = read_flags("payout-approver --to ana --approval payout:ok --why covering for bob --until 30d", &["--to", "--approval", "--why", "--until"]).unwrap();
        assert_eq!(words, ["payout-approver"]);
        assert_eq!(flags["to"], "ana");
        assert_eq!(flags["approval"], "payout:ok");
        assert_eq!(flags["why"], "covering for bob", "--why takes the words up to the next flag");
        assert_eq!(flags["until"], "30d");
        assert!(read_flags("x --flow", &["--flow"]).is_err(), "a flag without its value");
        assert!(read_flags("x --nope y", &["--flow"]).is_err(), "a flag it doesn't take");
        let (words, _) = read_flags("--to ana viewer", &["--to"]).unwrap();
        assert_eq!(words, ["viewer"], "the words wherever they are");
    }

    #[test]
    fn the_approval_settings_are_offered_as_their_flags() {
        assert_eq!(approvals_line(&json!({})), "--changes off --exclude-deployer off --agents on --starter on");
        assert_eq!(approvals_line(&json!({ "changes": 2, "exclude_deployer": true, "agents": false })), "--changes 2 --exclude-deployer on --agents off --starter on");
    }
}
