//! The admin page: the server's people and API keys, the folders and databases flows use, the
//! secrets they read, and the server's own settings (its permissions file, the grants held, the
//! sign-in provider, the store), each a list on the left and the picked row on the right, as a
//! deployment's page is. What the browser's admin page does, with the command line's words.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde_json::{json, Value};

use super::app::{App, Ask, List, Modal, Page, Said, Then, Tone};
use super::flow::Row;
use super::host::Res;
use crate::mcp::seg;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Part {
    #[default]
    People,
    Groups,
    Roles,
    Bindings,
    /// The access document's versions, the approval settings first.
    History,
    /// Your own agents and services.
    Yours,
    Keys,
    Folders,
    Databases,
    Mail,
    Feeds,
    Queues,
    /// The server's own secrets, and what its environment holds.
    Secrets,
    Storage,
    Policy,
    Server,
}

pub(crate) const PARTS: [(Part, &str); 16] = [
    (Part::People, "People"),
    (Part::Groups, "Groups"),
    (Part::Roles, "Roles"),
    (Part::Bindings, "Bindings"),
    (Part::History, "Access history"),
    (Part::Yours, "Your agents"),
    (Part::Keys, "API keys"),
    (Part::Folders, "Folders"),
    (Part::Databases, "Databases"),
    (Part::Mail, "Mail"),
    (Part::Feeds, "Feeds"),
    (Part::Queues, "Queues"),
    (Part::Secrets, "Secrets"),
    (Part::Storage, "Storage"),
    (Part::Policy, "Policy"),
    (Part::Server, "Server"),
];

/// The server section's rows.
pub(crate) const SERVER_ROWS: [&str; 5] = ["Grants held", "Sign-in provider", "Store", "Folder texts", "Host: memory and disks"];

#[derive(Default)]
pub(crate) struct AdminView {
    pub(crate) part: Part,
    pub(crate) sel: [usize; PARTS.len()],
    pub(crate) right: bool,
    pub(crate) scroll: usize,
    /// Every principal: people, agents and services.
    pub(crate) users: Vec<Value>,
    /// Every role (`GET /roles`): the built-in ones, and the ones an admin defined.
    pub(crate) roles: Vec<Value>,
    /// The access version the roles were read at: a change is made from it (`base`).
    pub(crate) roles_version: Value,
    /// The groups (`GET /groups`), and the access version they were read at.
    pub(crate) groups: Vec<Value>,
    pub(crate) groups_version: Value,
    /// Every binding, expired ones too (`GET /bindings`).
    pub(crate) bindings: Vec<Value>,
    /// The access document's versions, newest first (`GET /access/versions`).
    pub(crate) versions: Vec<Value>,
    /// The access document as it is (`GET /access`): its approval settings.
    pub(crate) access: Value,
    /// The caller's own agents and services (`GET /auth/owned`).
    pub(crate) owned: Vec<Value>,
    /// The agents' and services' tokens, each with its `principal`.
    pub(crate) tokens: Vec<Value>,
    pub(crate) folders: Vec<Value>,
    pub(crate) databases: Vec<Value>,
    pub(crate) mails: Vec<Value>,
    pub(crate) feeds: Vec<Value>,
    pub(crate) queues: Vec<Value>,
    /// The server's own secrets (`GET /admin/secrets`).
    pub(crate) secrets: Value,
    /// Each deployment's grant (`GET /admin/grants`), for who uses what; or why it isn't read.
    pub(crate) held: Vec<Value>,
    pub(crate) held_refused: Option<String>,
    /// What the host holds (`GET /admin/host`), and its policy (`GET /permissions`).
    pub(crate) host: Value,
    pub(crate) policy: Value,
    /// A folder on the server's machine, walked to bind (`GET /admin/folders/browse`).
    pub(crate) browse: Option<Value>,
    /// Why a part's list can't be read, by part: the caller may not see it (the server's
    /// refusal, as `may` says it ahead), or the read failed.
    pub(crate) refused: [Option<String>; PARTS.len()],
    /// The right pane, for the row it shows.
    pub(crate) pane: Option<(String, Vec<Row>)>,
}

impl AdminView {
    pub(crate) fn sel(&self) -> usize {
        self.sel[self.part as usize]
    }
}

/// What a key on a part does, as the action it takes: the routes' (can.rs's ROUTES), and for
/// a role given, the binding it makes too.
fn takes(p: Part, k: KeyCode, row: usize) -> &'static [&'static str] {
    match (p, k) {
        (Part::People, KeyCode::Enter) => &["people.manage", "people.assign"],
        (Part::People, KeyCode::Char('n' | 'a' | 'o' | 'e' | 'x' | 's' | 'D') | KeyCode::Delete) => &["people.manage"],
        (Part::People, KeyCode::Char('b')) => &["people.assign"],
        (Part::Keys, KeyCode::Char('n' | 'D') | KeyCode::Delete) => &["people.manage"],
        (Part::Folders | Part::Databases, KeyCode::Char('n')) => &["resource.bind"],
        (Part::Folders | Part::Databases, KeyCode::Char('D') | KeyCode::Delete) => &["resource.unbind"],
        (Part::Server, KeyCode::Enter) => match row {
            1 => &["people.manage"],
            2 | 3 => &["host.operate"],
            _ => &[],
        },
        _ => &[],
    }
}

/// What reading each of the server part's rows takes.
fn row_sees(row: usize) -> &'static str {
    match row {
        1 => "people.view",
        _ => "host.view",
    }
}

/// What a dialog on this page asks for.
pub(crate) enum AdminAsk {
    Invite,
    Add,
    Token,
    Folder,
    Database,
    Role(String),
    Email(String),
    Owner(String),
    Oidc,
    /// A dialog of the access parts (access.rs).
    Access(super::access::AccessAsk),
    /// A dialog of the host parts (host.rs).
    Host(super::host::HostAsk),
}

/// What a confirmed dialog on this page does.
pub(crate) enum AdminDo {
    Disable(String, bool),
    Reset(String),
    RemoveUser(String),
    /// A principal's token: its principal, its id.
    Revoke(String, String),
    Unbind { res: Res, name: String, deployment: Option<String> },
    ReloadPolicy,
    /// A dry run first: what it would remove, then the sweep, confirmed.
    SweepStore,
    SweepStoreNow,
    SweepFolders,
    /// A confirmed dialog of the host parts (host.rs).
    Host(super::host::HostDo),
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

/// The roles a principal of `kind` may hold, as `GET /roles` lists them: `viewer, editor,
/// admin or payout-approver`.
fn roles_of(roles: &[Value], kind: &Value) -> String {
    let names: Vec<String> = roles.iter().filter(|r| r["kinds"].as_array().is_some_and(|k| k.contains(kind))).map(|r| s(&r["name"])).collect();
    match names.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => "-".into(),
    }
}

/// The role bound to a principal on the host itself, not through a group: what `PATCH /people`
/// sets.
fn host_role(u: &Value) -> String {
    let own = u["bindings"].as_array().into_iter().flatten().find(|b| b["scope"] == "host" && b["group"].is_null() && b["expired"] != true);
    own.map(|b| s(&b["role"])).unwrap_or_default()
}

/// What a principal holds, a row a binding: its id (Bindings unbinds it), its role, where, and
/// through which group and until when.
fn held(u: &Value) -> Vec<Row> {
    let bindings = u["bindings"].as_array().cloned().unwrap_or_default();
    let mut rows = vec![text(Tone::Head, format!("Holds ({})", bindings.len()))];
    if bindings.is_empty() {
        rows.push(text(Tone::Dim, "  nothing: b binds a role"));
    }
    rows.extend(bindings.iter().map(|b| {
        let via = b["group"].as_str().map(|g| format!(", through {g}")).unwrap_or_default();
        let until = b["expires_at"].as_u64().map(|at| format!(", until {}", crate::cmdline::when(Some(at)))).unwrap_or_default();
        let tone = if b["expired"] == true { Tone::Dim } else { Tone::Plain };
        text(tone, format!("  #{} {} {}{via}{until}{}", b["id"], s(&b["role"]), crate::accesscmd::place(&b["scope"]), if b["expired"] == true { " (expired)" } else { "" }))
    }));
    rows
}

/// A folder listing's rows, as the walk lists them: its parent (`..`), its folders, then the
/// machine's drives or roots; each with the path it opens.
pub(crate) fn browse_rows(b: &Value) -> Vec<(String, String)> {
    let mut rows = vec![];
    if let Some(parent) = b["parent"].as_str() {
        rows.push(("..".to_string(), parent.to_string()));
    }
    for (k, mark) in [("folders", ""), ("roots", "⌂ ")] {
        rows.extend(b[k].as_array().into_iter().flatten().filter(|f| k == "folders" || f["path"] != b["path"]).map(|f| (format!("{mark}{}", s(&f["name"])), s(&f["path"]))));
    }
    rows
}

fn only(v: &Value) -> String {
    v["deployment"].as_str().map(|d| format!("?deployment={}", seg(d))).unwrap_or_default()
}

impl App {
    pub(crate) fn admin_rows(&self) -> usize {
        let a = &self.admin;
        match a.part {
            Part::People => a.users.len(),
            Part::Keys => a.tokens.len(),
            Part::Folders => match &a.browse {
                Some(b) => browse_rows(b).len(),
                None => a.folders.len(),
            },
            Part::Databases => a.databases.len(),
            Part::Server => SERVER_ROWS.len(),
            p @ (Part::Mail | Part::Feeds | Part::Queues | Part::Secrets | Part::Storage | Part::Policy) => self.host_part_rows(p),
            p => self.access_part_rows(p),
        }
    }

    /// Read the server's lists again, then the right pane.
    pub(crate) fn admin_load(&mut self) {
        let Some(c) = self.client.as_ref() else { return };
        // Each part's list, read only by whoever may see it: a part they may not says why, the
        // others are as an admin sees them.
        let list = |path: &str, action: &str| match c.may(action, None) {
            Ok(()) => c.get(path).map(|v| v.as_array().cloned().unwrap_or_default()),
            Err(why) => Err(why),
        };
        let (people, folders, databases) = (list("/people", "people.view"), list("/admin/folders", "resource.view"), list("/admin/databases", "resource.view"));
        let mut refused: [Option<String>; PARTS.len()] = Default::default();
        for (parts, read) in [(&[Part::People, Part::Keys][..], people.as_ref().err()), (&[Part::Folders], folders.as_ref().err()), (&[Part::Databases], databases.as_ref().err())] {
            for p in parts {
                refused[*p as usize] = read.cloned();
            }
        }
        let users = people.unwrap_or_default();
        self.admin.tokens = users.iter().flat_map(|p| p["tokens"].as_array().cloned().unwrap_or_default()).collect();
        (self.admin.users, self.admin.folders, self.admin.databases) = (users, folders.unwrap_or_default(), databases.unwrap_or_default());
        self.admin.refused = refused;
        self.access_part_load();
        self.host_part_load();
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
        let key = format!("{:?} {i}{}", a.part, a.browse.as_ref().map(|b| format!(" {}", b["path"])).unwrap_or_default());
        if a.pane.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let lines = |r: Result<Value, String>, f: fn(&Value) -> Vec<String>| -> Vec<Row> {
            match r {
                Ok(v) => f(&v).into_iter().map(|l| text(Tone::Plain, l)).collect(),
                Err(e) => vec![text(Tone::Bad, e)],
            }
        };
        // A row the caller may not read says why, in place of what it would read.
        let no = |why: String| vec![text(Tone::Bad, format!("It can't be shown here: {why}")), text(Tone::Dim, "Ask an admin for a role that holds it.")];
        let server_refused = if a.part == Part::Server { c.may(row_sees(i), None).err() } else { None };
        let rows: Vec<Row> = match a.part {
            _ if a.refused[a.part as usize].is_some() => no(a.refused[a.part as usize].clone().unwrap_or_default()),
            Part::Server if server_refused.is_some() => no(server_refused.unwrap_or_default()),
            Part::People => match a.users.get(i) {
                Some(u) if u["kind"] != "person" => [
                    vec![text(Tone::Head, s(&u["name"])), field("Kind", if u["kind"] == "agent" { "an agent" } else { "a service" })],
                    held(u),
                    vec![Row::Field("Owner".into(), format!("{}{}", s(&u["owner"]), if u["owner_disabled"] == true { " (disabled: its tokens are refused)" } else { "" }), if u["owner_disabled"] == true { Tone::Bad } else { Tone::Plain }),
                    field("API keys", u["tokens"].as_array().map_or(0, Vec::len).to_string()),
                    Row::Field("State".into(), if u["disabled"] == true { "disabled: its keys are refused".into() } else { "active".into() }, if u["disabled"] == true { Tone::Warn } else { Tone::Plain }),
                    field("Since", crate::mcp::when(&u["created_at"])),
                    field("By", s(&u["created_by"])),
                    text(Tone::Dim, "It signs in with its own API keys. enter: role · b: bind a role · o: owner · x: disable or enable · del: remove")],
                ]
                .concat(),
                Some(u) => [
                    vec![text(Tone::Head, s(&u["name"])), field("Kind", "a person")],
                    held(u),
                    vec![field("Email", s(&u["email"])),
                    Row::Field("State".into(), if u["disabled"] == true { "disabled: can't sign in".into() } else if u["invited"] == true { "invited, not joined yet".into() } else { "active".into() }, if u["disabled"] == true { Tone::Warn } else { Tone::Plain }),
                    field("Since", crate::mcp::when(&u["created_at"])),
                    field("By", s(&u["created_by"])),
                    text(Tone::Dim, "enter: role · b: bind a role · e: email · x: disable or enable · s: a new link to set a password · del: remove")],
                ]
                .concat(),
                None => vec![text(Tone::Dim, "n invites someone; a adds an agent or a service.")],
            },
            Part::Keys => match a.tokens.get(i) {
                Some(t) => vec![
                    text(Tone::Head, s(&t["label"])),
                    field("Of", s(&t["principal"])),
                    field("Id", s(&t["id"])),
                    field("Made", crate::mcp::when(&t["created_at"])),
                    field("By", s(&t["created_by"])),
                    text(Tone::Dim, "Its agent or service signs in with it (HOLON_TOKEN). Its value was shown once, when it was made."),
                    text(Tone::Dim, "del: revoke it"),
                ],
                None => vec![text(Tone::Dim, "n makes an API key for an agent or a service.")],
            },
            Part::Folders if a.browse.is_some() => {
                let b = a.browse.as_ref().unwrap_or(&Value::Null);
                let mut rows = vec![text(Tone::Head, s(&b["path"])), text(Tone::Plain, "A folder on the server's machine, to bind.")];
                if b["more"] == true {
                    rows.push(text(Tone::Dim, "It holds more folders than are listed."));
                }
                rows.push(text(Tone::Dim, "enter: open the folder picked · ←: its parent · n: bind the folder shown · esc: stop browsing"));
                rows
            }
            Part::Folders => match a.folders.get(i) {
                Some(f) => [vec![
                    text(Tone::Head, s(&f["name"])),
                    field("Path", s(&f["path"])),
                    field("Access", s(&f["access"])),
                    field("For", scope(f)),
                    field("Keeps in runs", s(&f["keep"])),
                    field("Holds at most", f["cap"].as_u64().map(crate::folders::size_text).unwrap_or_else(|| "no cap".into())),
                    field("Shared", if f["shared"] == true { "with other servers: a commit locks a file in it" } else { "no" }),
                    field("Lock", if f["lock"] == "run" { "for each run's whole length" } else { "at commit" })],
                    self.used_by(Res::Folder, f),
                    vec![text(Tone::Dim, "del: unbind it (its files stay where they are)")],
                ]
                .concat(),
                None => vec![text(Tone::Dim, "n binds a folder: a name flows use, a real folder here; b browses the server's folders for one.")],
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
                    rows.extend(self.used_by(Res::Database, d));
                    rows.push(text(Tone::Dim, "enter: its tables · del: unbind it (the database stays where it is)"));
                    rows
                }
                None => vec![text(Tone::Dim, "n binds a database: a name flows use (database shop), a file or a connection here.")],
            },
            Part::Server => match i {
                0 => {
                    let mut rows = vec![text(Tone::Head, "What each deployment holds")];
                    rows.extend(lines(c.get("/admin/grants"), crate::grantcmd::held_lines));
                    rows
                }
                1 => {
                    let mut rows = vec![text(Tone::Head, "Signing in through an identity provider (OIDC)")];
                    match c.get("/admin/oidc") {
                        Ok(v) if v["settings"].is_object() => rows.extend([field("Issuer", s(&v["settings"]["issuer"])), field("Client id", s(&v["settings"]["client_id"]))]),
                        Ok(_) => rows.push(text(Tone::Plain, "None: people sign in with a password.")),
                        Err(e) => rows.push(text(Tone::Bad, e)),
                    }
                    rows.push(text(Tone::Dim, "enter: set it, or clear it"));
                    rows
                }
                2 => {
                    let mut rows = vec![text(Tone::Head, "The store"), text(Tone::Plain, "Values nothing kept names any more: no run, version or state.")];
                    rows.extend(lines(c.get("/admin/host"), |v| crate::storage::sweep_lines(&v["sweeps"], "store", "values")));
                    rows.push(text(Tone::Dim, "enter: count what a sweep would remove, then ask; holon policy --sweep-every sets how often"));
                    rows
                }
                4 => {
                    let mut rows = vec![text(Tone::Head, "What the host holds: memory, its disks, and what it keeps on them")];
                    rows.extend(lines(c.get("/admin/host"), crate::storage::host_lines));
                    rows.push(text(Tone::Dim, "enter: count again; holon host says the same"));
                    rows
                }
                _ => {
                    let mut rows = vec![text(Tone::Head, "Folder texts"), text(Tone::Plain, "The file texts no live run, kept run or folder names.")];
                    rows.extend(lines(c.get("/admin/host"), |v| crate::storage::sweep_lines(&v["sweeps"], "folders", "texts")));
                    rows.push(text(Tone::Dim, "enter: remove them"));
                    rows
                }
            },
            p @ (Part::Mail | Part::Feeds | Part::Queues | Part::Secrets | Part::Storage | Part::Policy) => self.host_part_pane(p, i),
            p => self.access_part_pane(p, i),
        };
        self.admin.pane = Some((key, rows));
        self.admin.scroll = 0;
    }

    /// Whether the caller may use key `k` on the part shown: the part is shown to them, and they
    /// hold what the key takes, as `may` says it ahead. A key they may not use isn't offered.
    pub(crate) fn admin_may(&self, k: KeyCode) -> bool {
        let (part, i) = (self.admin.part, self.admin.sel());
        let Some(c) = self.client.as_ref() else { return false };
        // A key for one secret, on the row that is none (the environment's), does nothing.
        let secrets = self.admin.secrets["secrets"].as_array().map_or(0, Vec::len);
        if part == Part::Secrets && i >= secrets && matches!(k, KeyCode::Enter | KeyCode::Char('t' | 'R') | KeyCode::Delete) {
            return false;
        }
        self.admin.refused[part as usize].is_none() && self.takes_all(part, k, i).all(|a| c.may(a, None).is_ok())
    }

    /// What key `k` on row `row` of part `p` takes, across the admin page's parts.
    fn takes_all(&self, p: Part, k: KeyCode, row: usize) -> impl Iterator<Item = &'static str> {
        let secrets = self.admin.secrets["secrets"].as_array().map_or(0, Vec::len);
        // Browsing, a folder's keys are the walk's: n binds, as the part's n does.
        let k = if p == Part::Folders && self.admin.browse.is_some() && k != KeyCode::Char('n') { KeyCode::Null } else { k };
        takes(p, k, row).iter().chain(super::access::takes(p, k, row)).chain(super::host::takes(p, k, row, secrets)).copied()
    }

    /// The folder dialog, as holon folder add takes it, `value` already in it.
    fn ask_folder(&mut self, value: String) {
        self.ask(
            "Bind a folder",
            &["As holon folder add takes it: a name flows use, the folder's path, then any of", "--read-only  --for SLUG  --keep hash|30d  --cap SIZE  --shared  --lock run", "inbox ~/holon-inbox --read-only"],
            value,
            AdminAsk::Folder,
        );
    }

    /// Walk to folder `path` on the server's machine (none: its home folder), to bind one.
    fn browse(&mut self, path: Option<&str>) {
        let Some(c) = self.client.as_ref() else { return };
        let at = path.map(|p| format!("?path={}", seg(p))).unwrap_or_default();
        match c.get(&format!("/admin/folders/browse{at}")) {
            Ok(v) => {
                self.admin.browse = Some(v);
                self.admin.sel[Part::Folders as usize] = 0;
                self.admin.pane = None;
                self.admin_pane();
            }
            Err(e) => self.fail(e),
        }
    }

    /// A key while browsing: whether it was the walk's.
    fn browse_key(&mut self, k: KeyCode, i: usize) -> bool {
        let Some(b) = self.admin.browse.clone() else { return false };
        match k {
            KeyCode::Enter | KeyCode::Right => {
                if let Some((_, path)) = browse_rows(&b).into_iter().nth(i) {
                    self.browse(Some(&path));
                }
            }
            KeyCode::Left | KeyCode::Backspace => {
                if let Some(parent) = b["parent"].as_str() {
                    self.browse(Some(parent));
                }
            }
            KeyCode::Char('n') => {
                if self.client.as_ref().is_some_and(|c| c.may("resource.bind", None).is_ok()) {
                    let path = s(&b["path"]);
                    let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_lowercase().replace(' ', "-")).unwrap_or_default();
                    self.admin.browse = None;
                    self.admin.pane = None;
                    self.admin_pane();
                    self.ask_folder(format!("{name} {path}"));
                }
            }
            KeyCode::Esc => {
                self.admin.browse = None;
                self.admin.sel[Part::Folders as usize] = 0;
                self.admin.pane = None;
                self.admin_pane();
            }
            _ => return false,
        }
        true
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

    pub(super) fn ask(&mut self, title: impl Into<String>, lines: &[&str], value: impl Into<String>, then: AdminAsk) {
        self.modal = Some(Modal::input(title.into(), lines.iter().map(|l| l.to_string()).collect(), value.into(), false, Ask::Admin(then)));
    }

    pub(super) fn sure(&mut self, title: impl Into<String>, lines: &[&str], then: AdminDo) {
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
        if self.admin.part == Part::Folders && self.admin.browse.is_some() && self.browse_key(k.code, i) {
            return;
        }
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
        // A key the caller may not use says the server's refusal, and sends nothing.
        let part = self.admin.part;
        let refusal = self.client.as_ref().and_then(|c| self.takes_all(part, k.code, i).find_map(|a| c.may(a, None).err()));
        if let Some(why) = refusal {
            return self.fail(why);
        }
        if matches!(part, Part::Groups | Part::Roles | Part::Bindings | Part::History | Part::Yours) {
            return self.access_part_key(part, k.code, i);
        }
        if matches!(part, Part::Mail | Part::Feeds | Part::Queues | Part::Secrets | Part::Storage | Part::Policy) {
            return self.host_part_key(part, k.code, i);
        }
        let user = self.admin.users.get(i).map(|u| (s(&u["name"]), u.clone()));
        match (self.admin.part, k.code) {
            (Part::People, KeyCode::Char('n')) => {
                let roles = roles_of(&self.admin.roles, &json!("person"));
                let first = format!("Their name, a role ({roles}) and, if they sign in through a provider, their email:");
                self.ask("Invite someone", &[first.as_str(), "ana editor ana@example.com", "They get a link to set a password; it is copied to your clipboard."], "", AdminAsk::Invite);
            }
            (Part::People, KeyCode::Char('a')) => {
                let (agent, service) = (roles_of(&self.admin.roles, &json!("agent")), roles_of(&self.admin.roles, &json!("service")));
                let first = format!("Its name, agent or service, a role (an agent: {agent}; a service: {service}) and, if not you, its owner:");
                self.ask("Add an agent or a service", &[first.as_str(), "refunds-bot agent agent ana", "It signs in with its own API keys: make one under API keys."], "", AdminAsk::Add);
            }
            (Part::Keys, KeyCode::Char('n')) => self.ask("A new API key", &["The agent or service it is for, then what it is for:", "ci nightly report", "The key is shown once, and copied to your clipboard."], "", AdminAsk::Token),
            (Part::Folders, KeyCode::Char('n')) => self.ask_folder(String::new()),
            (Part::Folders, KeyCode::Char('b')) => self.browse(None),
            (Part::Databases, KeyCode::Char('n')) => self.ask(
                "Bind a database",
                &["As holon database add takes it: a name flows use, a SQLite file's path or where its URL is kept (env:…, vault:…), then any of", "--read-only  --for SLUG  --lock run  --schema SQL (last)", "shop ~/shop.db --schema create table if not exists items (id integer primary key)"],
                "",
                AdminAsk::Database,
            ),
            (Part::People, KeyCode::Enter) => {
                if let Some((name, u)) = user {
                    let roles = format!("Their role on this host: {}.", roles_of(&self.admin.roles, &u["kind"]));
                    let elsewhere = "On one flow, flows with a tag or an approval point: b binds a role there.";
                    self.ask(format!("{name}'s role"), &[roles.as_str(), elsewhere], host_role(&u), AdminAsk::Role(name));
                }
            }
            (Part::People, KeyCode::Char('b')) => {
                if let Some((name, _)) = user {
                    self.ask_bind(&format!("--to {name} "));
                }
            }
            (Part::People, KeyCode::Char('o')) => {
                if let Some((name, u)) = user.filter(|(_, u)| u["kind"] != "person") {
                    self.ask(format!("{name}'s owner"), &["The person answerable for it:"], s(&u["owner"]), AdminAsk::Owner(name));
                }
            }
            (Part::People, KeyCode::Char('e')) => {
                if let Some((name, u)) = user.filter(|(_, u)| u["kind"] == "person") {
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
                if let Some((name, _)) = user.filter(|(_, u)| u["kind"] == "person") {
                    self.sure(format!("A new link for {name}?"), &["A link to set a new password; their sign-ins end now. It is copied to your clipboard."], AdminDo::Reset(name));
                }
            }
            (Part::People, KeyCode::Delete | KeyCode::Char('D')) => {
                if let Some((name, _)) = user {
                    self.sure(format!("Remove {name}?"), &["They go, with an agent's or a service's keys; what they deployed stays, and says who did it."], AdminDo::RemoveUser(name));
                }
            }
            (Part::Keys, KeyCode::Delete | KeyCode::Char('D')) => {
                if let Some(t) = self.admin.tokens.get(i) {
                    let (of, id, label) = (s(&t["principal"]), s(&t["id"]), s(&t["label"]));
                    self.sure(format!("Revoke {of}'s {label}?"), &["What signs in with it is refused from now on."], AdminDo::Revoke(of, id));
                }
            }
            (Part::Folders | Part::Databases, KeyCode::Delete | KeyCode::Char('D')) => {
                let database = self.admin.part == Part::Databases;
                let res = if database { Res::Database } else { Res::Folder };
                let list = if database { &self.admin.databases } else { &self.admin.folders };
                if let Some(v) = list.get(i) {
                    let (name, deployment) = (s(&v["name"]), v["deployment"].as_str().map(str::to_string));
                    let line = if database { "Flows that use it can't reach it until it is bound again; the database stays where it is." } else { "Flows that use it can't reach it until it is bound again; its files stay where they are." };
                    self.sure(format!("Unbind {name}?"), &[line], AdminDo::Unbind { res, name, deployment });
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
            (Part::Server, KeyCode::Enter) => match i {
                1 => {
                    let Some(c) = self.client.as_ref() else { return };
                    let now = c.get("/admin/oidc").ok().filter(|v| v["settings"].is_object()).map(|v| format!("{} {}", s(&v["settings"]["issuer"]), s(&v["settings"]["client_id"]))).unwrap_or_default();
                    self.ask("Sign-in provider", &["The issuer's URL and this server's client id there (empty: none, a password only):", "https://login.example.com my-holon"], now, AdminAsk::Oidc);
                }
                2 => self.admin_do(AdminDo::SweepStore),
                3 => self.sure("Remove the folder texts nothing names?", &["Texts a live run, a kept run or a folder still names stay."], AdminDo::SweepFolders),
                4 => self.admin_pane(),
                _ => {}
            },
            _ => {}
        }
    }

    /// After a change: the lists again, and what happened.
    pub(super) fn admin_done(&mut self, r: Result<Value, String>, said: impl FnOnce(&Value) -> String) {
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
    pub(super) fn show_once(&mut self, title: String, what: &str, secret: String, after: &str) {
        self.to_clipboard = Some(secret.clone());
        self.modal = Some(Modal::Lines { title, lines: vec![(Tone::Plain, format!("{what}, copied to your clipboard:")), (Tone::Head, secret), (Tone::Dim, after.to_string())], scroll: 0 });
    }

    pub(crate) fn admin_do(&mut self, d: AdminDo) {
        let Some(c) = self.client.as_ref() else { return };
        match d {
            AdminDo::Disable(name, off) => {
                let r = c.patch(&format!("/people/{}", seg(&name)), json!({ "disabled": off }));
                self.admin_done(r, |_| format!("{name}: {}", if off { "disabled" } else { "enabled" }));
            }
            AdminDo::Reset(name) => match c.post(&format!("/people/{}/reset", seg(&name)), json!({})) {
                Ok(v) => {
                    let link = crate::accountscmd::invite_link(&c.server(), v["invite"].as_str().unwrap_or_default());
                    self.admin_load();
                    self.show_once(format!("A new link for {name}"), "The link to set a new password", link, "It works once. Send it to them yourself.");
                }
                Err(e) => self.fail(e),
            },
            AdminDo::RemoveUser(name) => {
                let r = c.delete(&format!("/people/{}", seg(&name)));
                self.admin_done(r, |_| format!("removed {name}"));
            }
            AdminDo::Revoke(of, id) => {
                let r = c.delete(&format!("/people/{}/tokens/{}", seg(&of), seg(&id)));
                self.admin_done(r, |_| format!("revoked {of}'s {id}"));
            }
            AdminDo::Unbind { res, name, deployment } => {
                let only = deployment.map(|d| format!("?deployment={}", seg(&d))).unwrap_or_default();
                let r = c.delete(&format!("/admin/{}/{}{only}", res.path(), seg(&name)));
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
            AdminDo::Host(d) => self.host_part_do(d),
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
                match c.post("/people", json!({ "name": name, "kind": "person", "role": role, "email": rest.first() })) {
                    Ok(v) => {
                        let link = crate::accountscmd::invite_link(&c.server(), v["invite"].as_str().unwrap_or_default());
                        self.admin_load();
                        self.show_once(format!("{name} is invited"), "Their link to set a password", link, "It works once. Send it to them yourself.");
                    }
                    Err(e) => self.fail(e),
                }
            }
            AdminAsk::Add => {
                let [name, kind, role, owner @ ..] = words.as_slice() else { return self.fail("a name, agent or service, and a role: refunds-bot agent agent") };
                let r = c.post("/people", json!({ "name": name, "kind": kind, "role": role, "owner": owner.first() }));
                self.admin_done(r, |v| format!("added {} ({}, owned by {})", s(&v["name"]), s(&v["kind"]), s(&v["owner"])));
            }
            AdminAsk::Token => {
                let [of, label @ ..] = words.as_slice() else { return self.fail("the agent or service, then what it is for: ci nightly report") };
                let label = if label.is_empty() { "made in holon's workbench".to_string() } else { label.join(" ") };
                match c.post(&format!("/people/{}/tokens", seg(of)), json!({ "label": label })) {
                    Ok(v) => {
                        let token = v["token"].as_str().unwrap_or_default().to_string();
                        self.admin_load();
                        self.show_once(format!("{of}: an API key"), "The key", token, "It is shown only now: keep it where it reads it (HOLON_TOKEN).");
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
                let role = value.trim().to_string();
                let r = c.patch(&format!("/people/{}", seg(&name)), json!({ "role": role }));
                self.admin_done(r, |_| format!("{name} holds {role} on this host now"));
            }
            AdminAsk::Email(name) => {
                let r = c.patch(&format!("/people/{}", seg(&name)), json!({ "email": value.trim() }));
                self.admin_done(r, |_| format!("{name}'s email changed"));
            }
            AdminAsk::Owner(name) => {
                let r = c.patch(&format!("/people/{}", seg(&name)), json!({ "owner": value.trim() }));
                self.admin_done(r, |v| format!("{name} is owned by {} now", s(&v["owner"])));
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
            AdminAsk::Access(ask) => self.access_part_answer(ask, value),
            AdminAsk::Host(ask) => self.host_part_answer(ask, value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_is_offered_the_roles_it_may_hold_and_its_own_host_role() {
        let roles = [json!({ "name": "viewer", "kinds": ["person", "agent", "service"] }), json!({ "name": "admin", "kinds": ["person"] }), json!({ "name": "payout-approver", "kinds": ["person"] })];
        assert_eq!(roles_of(&roles, &json!("person")), "viewer, admin or payout-approver");
        assert_eq!(roles_of(&roles, &json!("agent")), "viewer");
        assert_eq!(roles_of(&[], &json!("agent")), "-");
        let held = |bindings: Value| host_role(&json!({ "bindings": bindings }));
        let through = json!({ "role": "editor", "scope": "host", "group": "ops", "expired": false });
        let on_a_flow = json!({ "role": "editor", "scope": { "flows": { "slug": "payout" } }, "group": null, "expired": false });
        let gone = json!({ "role": "admin", "scope": "host", "group": null, "expired": true });
        let own = json!({ "role": "payout-approver", "scope": "host", "group": null, "expired": false });
        assert_eq!(held(json!([through, on_a_flow, gone, own])), "payout-approver");
        assert_eq!(held(json!([through, on_a_flow])), "", "a group's, or a flow's, isn't the host role PATCH sets");
    }
}
