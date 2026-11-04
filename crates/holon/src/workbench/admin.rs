//! The admin page: the server's people and API keys, the folders and databases flows use, the
//! secrets they read, and the server's own settings (its permissions file, the grants held, the
//! sign-in provider, the store), each a list on the left and the picked row on the right, as a
//! deployment's page is. What the browser's admin page does, with the command line's words.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde_json::{json, Value};

use super::app::{App, Ask, List, Modal, Page, Said, Then, Tone};
use super::flow::{Row, Section};
use crate::mcp::seg;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Part {
    #[default]
    People,
    Keys,
    Folders,
    Databases,
    Secrets,
    Server,
}

pub(crate) const PARTS: [(Part, &str); 6] = [(Part::People, "People"), (Part::Keys, "API keys"), (Part::Folders, "Folders"), (Part::Databases, "Databases"), (Part::Secrets, "Secrets"), (Part::Server, "Server")];

/// The server section's rows.
pub(crate) const SERVER_ROWS: [&str; 5] = ["Permissions (holon.toml)", "Grants held", "Sign-in provider", "Store", "Folder texts"];

#[derive(Default)]
pub(crate) struct AdminView {
    pub(crate) part: Part,
    pub(crate) sel: [usize; 6],
    pub(crate) right: bool,
    pub(crate) scroll: usize,
    pub(crate) users: Vec<Value>,
    pub(crate) tokens: Vec<Value>,
    pub(crate) folders: Vec<Value>,
    pub(crate) databases: Vec<Value>,
    /// Why the server's lists couldn't be read (not an admin, say).
    pub(crate) error: Option<String>,
    /// The right pane, for the row it shows.
    pub(crate) pane: Option<(String, Vec<Row>)>,
}

impl AdminView {
    pub(crate) fn sel(&self) -> usize {
        self.sel[self.part as usize]
    }
}

/// What a dialog on this page asks for.
pub(crate) enum AdminAsk {
    Invite,
    Token,
    Folder,
    Database,
    Role(String),
    Email(String),
    Oidc,
}

/// What a confirmed dialog on this page does.
pub(crate) enum AdminDo {
    Disable(String, bool),
    Reset(String),
    RemoveUser(String),
    Revoke(String),
    Unbind { database: bool, name: String, deployment: Option<String> },
    ReloadPolicy,
    /// A dry run first: what it would remove, then the sweep, confirmed.
    SweepStore,
    SweepStoreNow,
    SweepFolders,
}

fn field(k: &str, v: impl Into<String>) -> Row {
    Row::Field(k.to_string(), v.into(), Tone::Plain)
}

fn text(t: Tone, s: impl Into<String>) -> Row {
    Row::Text(t, s.into())
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("-").to_string()
}

/// A binding's scope: for every flow, or for one deployment.
fn scope(v: &Value) -> String {
    v["deployment"].as_str().map(|d| format!("{d} only")).unwrap_or_else(|| "every flow".into())
}

/// A path as the server's machine sees it, as `holon folder add` sends it: whole, from the folder
/// the editor is in, `~` as the home folder.
fn here(path: &str) -> String {
    let home = || std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(std::path::PathBuf::from).unwrap_or_default();
    let p = match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => home().join(rest),
        None if path == "~" => home(),
        None => std::path::PathBuf::from(path),
    };
    std::path::absolute(&p).unwrap_or(p).display().to_string()
}

fn only(v: &Value) -> String {
    v["deployment"].as_str().map(|d| format!("?deployment={}", seg(d))).unwrap_or_default()
}

impl App {
    fn deployments(&self) -> Vec<String> {
        self.client.as_ref().map(|c| c.deployments.iter().filter_map(|d| d["slug"].as_str().map(str::to_string)).collect()).unwrap_or_default()
    }

    pub(crate) fn admin_rows(&self) -> usize {
        let a = &self.admin;
        match a.part {
            Part::People => a.users.len(),
            Part::Keys => a.tokens.len(),
            Part::Folders => a.folders.len(),
            Part::Databases => a.databases.len(),
            Part::Secrets => 1 + self.deployments().len(),
            Part::Server => SERVER_ROWS.len(),
        }
    }

    /// Read the server's lists again, then the right pane.
    pub(crate) fn admin_load(&mut self) {
        let Some(c) = self.client.as_ref() else { return };
        let list = |path: &str| c.get(path).map(|v| v.as_array().cloned().unwrap_or_default());
        match (list("/admin/users"), list("/admin/tokens"), list("/admin/folders"), list("/admin/databases")) {
            (Ok(u), Ok(t), Ok(f), Ok(d)) => {
                (self.admin.users, self.admin.tokens, self.admin.folders, self.admin.databases) = (u, t, f, d);
                self.admin.error = None;
            }
            (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), _) | (.., Err(e)) => self.admin.error = Some(e),
        }
        let n = self.admin_rows();
        let p = self.admin.part as usize;
        self.admin.sel[p] = self.admin.sel[p].min(n.saturating_sub(1));
        // The same row, read again: where it was scrolled to stays.
        let (key, scroll) = (self.admin.pane.take().map(|(k, _)| k), self.admin.scroll);
        self.admin_pane();
        if self.admin.pane.as_ref().map(|(k, _)| k) == key.as_ref() {
            self.admin.scroll = scroll;
        }
    }

    /// The right pane for the row picked.
    fn admin_pane(&mut self) {
        let Some(c) = self.client.as_ref() else { return };
        let a = &self.admin;
        let i = a.sel();
        let key = format!("{:?} {i}", a.part);
        if a.pane.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let lines = |r: Result<Value, String>, f: fn(&Value) -> Vec<String>| -> Vec<Row> {
            match r {
                Ok(v) => f(&v).into_iter().map(|l| text(Tone::Plain, l)).collect(),
                Err(e) => vec![text(Tone::Bad, e)],
            }
        };
        let rows: Vec<Row> = match a.part {
            Part::People => match a.users.get(i) {
                Some(u) => vec![
                    text(Tone::Head, s(&u["name"])),
                    field("Role", s(&u["role"])),
                    field("Email", s(&u["email"])),
                    Row::Field("State".into(), if u["disabled"] == true { "disabled: can't sign in".into() } else if u["invited"] == true { "invited, not joined yet".into() } else { "active".into() }, if u["disabled"] == true { Tone::Warn } else { Tone::Plain }),
                    field("Since", crate::mcp::when(&u["created_at"])),
                    field("By", s(&u["created_by"])),
                    text(Tone::Dim, "enter: role · e: email · x: disable or enable · s: a new link to set a password · del: remove"),
                ],
                None => vec![text(Tone::Dim, "n invites someone.")],
            },
            Part::Keys => match a.tokens.get(i) {
                Some(t) => vec![
                    text(Tone::Head, s(&t["label"])),
                    field("Id", s(&t["id"])),
                    field("Role", s(&t["role"])),
                    field("Made", crate::mcp::when(&t["created_at"])),
                    field("By", s(&t["created_by"])),
                    text(Tone::Dim, "A script signs in with it (HOLON_TOKEN). Its value was shown once, when it was made."),
                    text(Tone::Dim, "del: revoke it"),
                ],
                None => vec![text(Tone::Dim, "n makes an API key for a script or an agent.")],
            },
            Part::Folders => match a.folders.get(i) {
                Some(f) => vec![
                    text(Tone::Head, s(&f["name"])),
                    field("Path", s(&f["path"])),
                    field("Access", s(&f["access"])),
                    field("For", scope(f)),
                    field("Keeps in runs", s(&f["keep"])),
                    field("Holds at most", f["cap"].as_u64().map(crate::folders::size_text).unwrap_or_else(|| "no cap".into())),
                    field("Shared", if f["shared"] == true { "with other servers: a commit locks a file in it" } else { "no" }),
                    field("Lock", if f["lock"] == "run" { "for each run's whole length" } else { "at commit" }),
                    text(Tone::Dim, "del: unbind it (its files stay where they are)"),
                ],
                None => vec![text(Tone::Dim, "n binds a folder: a name flows use, a real folder here.")],
            },
            Part::Databases => match a.databases.get(i) {
                Some(d) => {
                    let mut rows = vec![text(Tone::Head, s(&d["name"])), field("Engine", s(&d["engine"]))];
                    for (k, label) in [("path", "File"), ("connection", "From"), ("address", "Address")] {
                        if let Some(v) = d[k].as_str() {
                            rows.push(field(label, v));
                        }
                    }
                    rows.extend([field("Access", s(&d["access"])), field("For", scope(d)), field("Lock", if d["lock"] == "run" { "for each run's whole length" } else { "at commit" })]);
                    rows.push(text(Tone::Dim, "enter: its tables · del: unbind it (the database stays where it is)"));
                    rows
                }
                None => vec![text(Tone::Dim, "n binds a database: a name flows use (database shop), a file or a connection here.")],
            },
            Part::Secrets if i == 0 => {
                let mut rows = vec![text(Tone::Head, "Read from the environment"), text(Tone::Dim, "A secret set as env:HOLON_SECRET_… reads one of these. Names only, never values.")];
                rows.extend(lines(c.get("/admin/secrets/env"), crate::secretcmd::env_lines));
                rows
            }
            Part::Secrets => {
                let slug = self.deployments().get(i - 1).cloned().unwrap_or_default();
                let mut rows = vec![text(Tone::Head, slug.clone())];
                rows.extend(lines(c.get(&format!("/deployments/{}/secrets", seg(&slug))), crate::secretcmd::list_lines));
                rows.push(text(Tone::Dim, "enter: set or change them, on its Access section"));
                rows
            }
            Part::Server => match i {
                0 => {
                    let mut rows = vec![text(Tone::Head, "What flows get without asking, and the most they may ask for")];
                    rows.extend(lines(c.get("/permissions"), crate::grantcmd::policy_lines));
                    rows.push(text(Tone::Dim, "enter: read holon.toml again and hold every deployment to it"));
                    rows
                }
                1 => {
                    let mut rows = vec![text(Tone::Head, "What each deployment holds")];
                    rows.extend(lines(c.get("/admin/grants"), crate::grantcmd::held_lines));
                    rows
                }
                2 => {
                    let mut rows = vec![text(Tone::Head, "Signing in through an identity provider (OIDC)")];
                    match c.get("/admin/oidc") {
                        Ok(v) if v["settings"].is_object() => rows.extend([field("Issuer", s(&v["settings"]["issuer"])), field("Client id", s(&v["settings"]["client_id"]))]),
                        Ok(_) => rows.push(text(Tone::Plain, "None: people sign in with a password.")),
                        Err(e) => rows.push(text(Tone::Bad, e)),
                    }
                    rows.push(text(Tone::Dim, "enter: set it, or clear it"));
                    rows
                }
                3 => {
                    let mut rows = vec![text(Tone::Head, "The store"), text(Tone::Plain, "Values nothing kept names any more: no run, version or state.")];
                    rows.extend(lines(c.get("/admin/host"), |v| crate::storage::sweep_lines(&v["sweeps"], "store", "values")));
                    rows.push(text(Tone::Dim, "enter: count what a sweep would remove, then ask; holon policy --sweep-every sets how often"));
                    rows
                }
                _ => {
                    let mut rows = vec![text(Tone::Head, "Folder texts"), text(Tone::Plain, "The file texts no live run, kept run or folder names.")];
                    rows.extend(lines(c.get("/admin/host"), |v| crate::storage::sweep_lines(&v["sweeps"], "folders", "texts")));
                    rows.push(text(Tone::Dim, "enter: remove them"));
                    rows
                }
            },
        };
        self.admin.pane = Some((key, rows));
        self.admin.scroll = 0;
    }

    pub(crate) fn admin_part(&mut self, p: Part) {
        self.admin.part = p;
        self.admin.right = false;
        self.admin_pane();
    }

    pub(crate) fn admin_pick(&mut self, list: List, i: usize, again: bool) {
        if list == List::AdminPane {
            self.admin.right = true;
            return;
        }
        let p = self.admin.part as usize;
        self.admin.sel[p] = i.min(self.admin_rows().saturating_sub(1));
        self.admin.right = false;
        self.admin_pane();
        if again {
            self.admin_key(KeyEvent::from(KeyCode::Enter));
        }
    }

    pub(crate) fn admin_scroll(&mut self, list: List, by: isize) {
        if list == List::AdminPane {
            self.admin.scroll = (self.admin.scroll as isize + by).max(0) as usize;
        } else {
            let p = self.admin.part as usize;
            self.admin.sel[p] = (self.admin.sel[p] as isize + by).clamp(0, self.admin_rows().saturating_sub(1) as isize) as usize;
            self.admin_pane();
        }
    }

    fn ask(&mut self, title: impl Into<String>, lines: &[&str], value: impl Into<String>, then: AdminAsk) {
        self.modal = Some(Modal::Input { title: title.into(), lines: lines.iter().map(|l| l.to_string()).collect(), value: value.into(), masked: false, then: Ask::Admin(then) });
    }

    fn sure(&mut self, title: impl Into<String>, lines: &[&str], then: AdminDo) {
        self.modal = Some(Modal::Confirm { title: title.into(), lines: lines.iter().map(|l| l.to_string()).collect(), then: Then::Admin(then) });
    }

    pub(crate) fn admin_key(&mut self, k: KeyEvent) {
        let at = PARTS.iter().position(|(p, _)| *p == self.admin.part).unwrap_or(0);
        let next = |by: usize| PARTS[(at + by) % PARTS.len()].0;
        if self.admin.right {
            match k.code {
                KeyCode::Up | KeyCode::Char('k') => self.admin.scroll = self.admin.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => self.admin.scroll += 1,
                KeyCode::PageUp => self.admin.scroll = self.admin.scroll.saturating_sub(10),
                KeyCode::PageDown => self.admin.scroll += 10,
                KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => self.admin.right = false,
                KeyCode::Tab => self.admin_part(next(1)),
                KeyCode::BackTab => self.admin_part(next(PARTS.len() - 1)),
                _ => {}
            }
            return;
        }
        let i = self.admin.sel();
        match k.code {
            KeyCode::Tab => return self.admin_part(next(1)),
            KeyCode::BackTab => return self.admin_part(next(PARTS.len() - 1)),
            KeyCode::Right | KeyCode::Char('l') => return self.admin.right = true,
            KeyCode::Up | KeyCode::Char('k') => return self.admin_scroll(List::Admin, -1),
            KeyCode::Down | KeyCode::Char('j') => return self.admin_scroll(List::Admin, 1),
            KeyCode::PageUp => return self.admin_scroll(List::Admin, -10),
            KeyCode::PageDown => return self.admin_scroll(List::Admin, 10),
            KeyCode::Home => return self.admin_scroll(List::Admin, isize::MIN / 2),
            KeyCode::End => return self.admin_scroll(List::Admin, isize::MAX / 2),
            KeyCode::Esc => return self.go(Page::Overview),
            _ => {}
        }
        let user = self.admin.users.get(i).map(|u| (s(&u["name"]), u.clone()));
        match (self.admin.part, k.code) {
            (Part::People, KeyCode::Char('n')) => self.ask("Invite someone", &["Their name, a role (viewer, editor or admin) and, if they sign in through a provider, their email:", "ana editor ana@example.com", "They get a link to set a password; it is copied to your clipboard."], "", AdminAsk::Invite),
            (Part::Keys, KeyCode::Char('n')) => self.ask("A new API key", &["A label and a role (viewer, editor, admin or agent):", "nightly-report viewer", "The key is shown once, and copied to your clipboard."], "", AdminAsk::Token),
            (Part::Folders, KeyCode::Char('n')) => self.ask(
                "Bind a folder",
                &["As holon folder add takes it: a name flows use, the folder's path, then any of", "--read-only  --for SLUG  --keep hash|30d  --cap SIZE  --shared  --lock run", "inbox ~/holon-inbox --read-only"],
                "",
                AdminAsk::Folder,
            ),
            (Part::Databases, KeyCode::Char('n')) => self.ask(
                "Bind a database",
                &["As holon database add takes it: a name flows use, a SQLite file's path or where its URL is kept (env:…, vault:…), then any of", "--read-only  --for SLUG  --lock run  --schema SQL (last)", "shop ~/shop.db --schema create table if not exists items (id integer primary key)"],
                "",
                AdminAsk::Database,
            ),
            (Part::People, KeyCode::Enter) => {
                if let Some((name, u)) = user {
                    self.ask(format!("{name}'s role"), &["viewer, editor or admin:"], s(&u["role"]), AdminAsk::Role(name));
                }
            }
            (Part::People, KeyCode::Char('e')) => {
                if let Some((name, u)) = user {
                    let email = u["email"].as_str().unwrap_or_default().to_string();
                    self.ask(format!("{name}'s email"), &["The email they sign in with through the identity provider (empty: none):"], email, AdminAsk::Email(name));
                }
            }
            (Part::People, KeyCode::Char('x')) => {
                if let Some((name, u)) = user {
                    let off = u["disabled"] != true;
                    let line = if off { "They can't sign in until enabled again; their sign-ins end now." } else { "They can sign in again." };
                    self.sure(format!("{} {name}?", if off { "Disable" } else { "Enable" }), &[line], AdminDo::Disable(name, off));
                }
            }
            (Part::People, KeyCode::Char('s')) => {
                if let Some((name, _)) = user {
                    self.sure(format!("A new link for {name}?"), &["A link to set a new password; their sign-ins end now. It is copied to your clipboard."], AdminDo::Reset(name));
                }
            }
            (Part::People, KeyCode::Delete | KeyCode::Char('D')) => {
                if let Some((name, _)) = user {
                    self.sure(format!("Remove {name}?"), &["Their account goes; what they deployed stays, and says who did it."], AdminDo::RemoveUser(name));
                }
            }
            (Part::Keys, KeyCode::Delete | KeyCode::Char('D')) => {
                if let Some(t) = self.admin.tokens.get(i) {
                    let (id, label) = (s(&t["id"]), s(&t["label"]));
                    self.sure(format!("Revoke {label}?"), &["What signs in with it is refused from now on."], AdminDo::Revoke(id));
                }
            }
            (Part::Folders | Part::Databases, KeyCode::Delete | KeyCode::Char('D')) => {
                let database = self.admin.part == Part::Databases;
                let list = if database { &self.admin.databases } else { &self.admin.folders };
                if let Some(v) = list.get(i) {
                    let (name, deployment) = (s(&v["name"]), v["deployment"].as_str().map(str::to_string));
                    let line = if database { "Flows that use it can't reach it until it is bound again; the database stays where it is." } else { "Flows that use it can't reach it until it is bound again; its files stay where they are." };
                    self.sure(format!("Unbind {name}?"), &[line], AdminDo::Unbind { database, name, deployment });
                }
            }
            (Part::Databases, KeyCode::Enter | KeyCode::Char('t')) => {
                let Some(d) = self.admin.databases.get(i).cloned() else { return };
                let Some(c) = self.client.as_ref() else { return };
                let name = s(&d["name"]);
                let lines: Vec<Said> = match c.get(&format!("/admin/databases/{}/tables{}", seg(&name), only(&d))) {
                    Ok(v) => {
                        let tables: Vec<Said> = v["tables"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|t| (Tone::Plain, t.to_string())).collect();
                        if tables.is_empty() { vec![(Tone::Dim, "no tables yet".into())] } else { tables }
                    }
                    Err(e) => vec![(Tone::Bad, e)],
                };
                self.modal = Some(Modal::Lines { title: format!("{name}'s tables"), lines, scroll: 0 });
            }
            (Part::Secrets, KeyCode::Enter) if i > 0 => {
                if let Some(slug) = self.deployments().get(i - 1).cloned() {
                    self.open_flow(&slug);
                    if self.page == Page::Flow {
                        self.flow_section(Section::Access);
                    }
                }
            }
            (Part::Server, KeyCode::Enter) => match i {
                0 => self.sure("Read holon.toml again?", &["Every deployment is held to it as it is now: one that asks for more than it allows now pauses."], AdminDo::ReloadPolicy),
                2 => {
                    let Some(c) = self.client.as_ref() else { return };
                    let now = c.get("/admin/oidc").ok().filter(|v| v["settings"].is_object()).map(|v| format!("{} {}", s(&v["settings"]["issuer"]), s(&v["settings"]["client_id"]))).unwrap_or_default();
                    self.ask("Sign-in provider", &["The issuer's URL and this server's client id there (empty: none, a password only):", "https://login.example.com my-holon"], now, AdminAsk::Oidc);
                }
                3 => self.admin_do(AdminDo::SweepStore),
                4 => self.sure("Remove the folder texts nothing names?", &["Texts a live run, a kept run or a folder still names stay."], AdminDo::SweepFolders),
                _ => {}
            },
            _ => {}
        }
    }

    /// After a change: the lists again, and what happened.
    fn admin_done(&mut self, r: Result<Value, String>, said: impl FnOnce(&Value) -> String) {
        match r {
            Ok(v) => {
                let line = said(&v);
                self.admin.pane = None;
                self.admin_load();
                self.say(line);
            }
            Err(e) => self.fail(e),
        }
    }

    /// A one-time secret (an invite link, a key): shown once, and copied.
    fn show_once(&mut self, title: String, what: &str, secret: String, after: &str) {
        self.to_clipboard = Some(secret.clone());
        self.modal = Some(Modal::Lines { title, lines: vec![(Tone::Plain, format!("{what}, copied to your clipboard:")), (Tone::Head, secret), (Tone::Dim, after.to_string())], scroll: 0 });
    }

    pub(crate) fn admin_do(&mut self, d: AdminDo) {
        let Some(c) = self.client.as_ref() else { return };
        match d {
            AdminDo::Disable(name, off) => {
                let r = c.post(&format!("/admin/users/{}", seg(&name)), json!({ "disabled": off }));
                self.admin_done(r, |_| format!("{name}: {}", if off { "disabled" } else { "enabled" }));
            }
            AdminDo::Reset(name) => match c.post(&format!("/admin/users/{}/reset", seg(&name)), json!({})) {
                Ok(v) => {
                    let link = crate::accountscmd::invite_link(&c.server(), v["invite"].as_str().unwrap_or_default());
                    self.admin_load();
                    self.show_once(format!("A new link for {name}"), "The link to set a new password", link, "It works once. Send it to them yourself.");
                }
                Err(e) => self.fail(e),
            },
            AdminDo::RemoveUser(name) => {
                let r = c.delete(&format!("/admin/users/{}", seg(&name)));
                self.admin_done(r, |_| format!("removed {name}"));
            }
            AdminDo::Revoke(id) => {
                let r = c.delete(&format!("/admin/tokens/{}", seg(&id)));
                self.admin_done(r, |_| format!("revoked {id}"));
            }
            AdminDo::Unbind { database, name, deployment } => {
                let only = deployment.map(|d| format!("?deployment={}", seg(&d))).unwrap_or_default();
                let r = c.delete(&format!("/admin/{}/{}{only}", if database { "databases" } else { "folders" }, seg(&name)));
                self.admin_done(r, |_| format!("unbound {name}"));
            }
            AdminDo::ReloadPolicy => {
                let r = c.post("/permissions/reload", json!({}));
                self.admin_done(r, |_| "read holon.toml again: every deployment is held to it".into());
            }
            AdminDo::SweepStore => match c.post("/admin/store/sweep", json!({ "dry_run": true })) {
                Ok(v) => {
                    let n = |k: &str| v[k]["values"].as_u64().unwrap_or(0);
                    let size = |k: &str| crate::folders::size_text(v[k]["bytes"].as_u64().unwrap_or(0));
                    if n("swept") == 0 {
                        return self.say(format!("nothing to sweep: the store keeps {} values ({})", n("kept"), size("kept")));
                    }
                    let lines = [format!("{} values ({}) nothing names any more.", n("swept"), size("swept")), format!("It keeps {} ({}), and spares {} put in the last hour.", n("kept"), size("kept"), n("spared"))];
                    self.modal = Some(Modal::Confirm { title: "Sweep the store?".into(), lines: lines.to_vec(), then: Then::Admin(AdminDo::SweepStoreNow) });
                }
                Err(e) => self.fail(e),
            },
            AdminDo::SweepStoreNow => {
                let r = c.post("/admin/store/sweep", json!({ "dry_run": false }));
                self.admin_done(r, |v| format!("swept {} values ({})", v["swept"]["values"].as_u64().unwrap_or(0), crate::folders::size_text(v["swept"]["bytes"].as_u64().unwrap_or(0))));
            }
            AdminDo::SweepFolders => {
                let r = c.post("/admin/folders/sweep", json!({}));
                self.admin_done(r, |v| format!("removed {} file texts ({})", v["texts"].as_u64().unwrap_or(0), crate::folders::size_text(v["bytes"].as_u64().unwrap_or(0))));
            }
        }
    }

    pub(crate) fn admin_answer(&mut self, a: AdminAsk, value: String) {
        let Some(c) = self.client.as_ref() else { return };
        let words: Vec<&str> = value.split_whitespace().collect();
        // A binding as `holon folder add` / `holon database add` take it: its words, then flags.
        let binding = |kind: &str| -> Result<(Vec<String>, Value), String> {
            let (head, schema) = match value.split_once("--schema") {
                Some((h, s)) => (h, Some(s.trim().trim_matches(['"', '\'']).to_string())),
                None => (value.as_str(), None),
            };
            let mut plain = vec![];
            let mut flags = json!({ "access": "read-write", "deployment": null });
            let mut it = head.split_whitespace();
            while let Some(w) = it.next() {
                let mut next = |flag: &str| it.next().map(str::to_string).ok_or_else(|| format!("{flag} takes a value"));
                match w {
                    "--read-only" => flags["access"] = json!("read"),
                    "--shared" => flags["shared"] = json!(true),
                    "--for" => flags["deployment"] = json!(next(w)?),
                    "--keep" => flags["keep"] = json!(match next(w)?.as_str() { "hash" => "hash".to_string(), d => format!("bytes for {d}") }),
                    "--cap" => flags["cap"] = json!(next(w)?),
                    "--lock" => flags["lock"] = json!(next(w)?),
                    f if f.starts_with("--") => return Err(format!("holon {kind} add takes no {f}")),
                    _ => plain.push(w.to_string()),
                }
            }
            if let Some(s) = schema {
                flags["schema"] = json!(s);
            }
            Ok((plain, flags))
        };
        match a {
            AdminAsk::Invite => {
                let [name, role, rest @ ..] = words.as_slice() else { return self.fail("a name and a role: ana editor") };
                match c.post("/admin/users", json!({ "name": name, "role": role, "email": rest.first() })) {
                    Ok(v) => {
                        let link = crate::accountscmd::invite_link(&c.server(), v["invite"].as_str().unwrap_or_default());
                        self.admin_load();
                        self.show_once(format!("{name} is invited"), "Their link to set a password", link, "It works once. Send it to them yourself.");
                    }
                    Err(e) => self.fail(e),
                }
            }
            AdminAsk::Token => {
                let [label, role] = words.as_slice() else { return self.fail("a label and a role: nightly-report viewer") };
                match c.post("/admin/tokens", json!({ "label": label, "role": role })) {
                    Ok(v) => {
                        let token = v["token"].as_str().unwrap_or_default().to_string();
                        self.admin_load();
                        self.show_once(format!("{label}: an API key"), "The key", token, "It is shown only now: keep it where the script reads it (HOLON_TOKEN).");
                    }
                    Err(e) => self.fail(e),
                }
            }
            AdminAsk::Folder => {
                let (plain, mut body) = match binding("folder") {
                    Ok(b) => b,
                    Err(e) => return self.fail(e),
                };
                let [name, path @ ..] = plain.as_slice() else { return self.fail("a name and a path: inbox C:\\data\\inbox") };
                if path.is_empty() {
                    return self.fail("and the folder's path");
                }
                body["name"] = json!(name);
                body["path"] = json!(here(&path.join(" ")));
                if body.get("keep").is_none() {
                    body["keep"] = json!("bytes");
                }
                let r = c.post("/admin/folders", body);
                self.admin_done(r, |v| format!("bound {} to {}", s(&v["name"]), s(&v["path"])));
            }
            AdminAsk::Database => {
                let (plain, mut body) = match binding("database") {
                    Ok(b) => b,
                    Err(e) => return self.fail(e),
                };
                let [name, place @ ..] = plain.as_slice() else { return self.fail("a name and a file or a connection: shop ~/shop.db") };
                let place = place.join(" ");
                if place.is_empty() {
                    return self.fail("and its file, or where its connection is kept");
                }
                body["name"] = json!(name);
                if place.starts_with("env:") || place.starts_with("vault:") {
                    body["connection"] = json!(place);
                } else {
                    body["path"] = json!(here(&place));
                }
                let r = c.post("/admin/databases", body);
                self.admin_done(r, |v| format!("bound {} ({})", s(&v["name"]), s(&v["engine"])));
            }
            AdminAsk::Role(name) => {
                let r = c.post(&format!("/admin/users/{}", seg(&name)), json!({ "role": value.trim() }));
                self.admin_done(r, |v| format!("{name} is {} now", s(&v["role"])));
            }
            AdminAsk::Email(name) => {
                let r = c.post(&format!("/admin/users/{}", seg(&name)), json!({ "email": value.trim() }));
                self.admin_done(r, |_| format!("{name}'s email changed"));
            }
            AdminAsk::Oidc => {
                let settings = match words.as_slice() {
                    [] => Value::Null,
                    [issuer, client_id] => json!({ "issuer": issuer, "client_id": client_id }),
                    _ => return self.fail("the issuer's URL and the client id, or nothing"),
                };
                let cleared = settings.is_null();
                let r = c.post("/admin/oidc", json!({ "settings": settings }));
                self.admin_done(r, |_| if cleared { "no sign-in provider: a password only".into() } else { "the sign-in provider is set".into() });
            }
        }
    }
}
