//! The admin page's host parts, as the web console's host pages: the mail, feeds and queues flows
//! use (bound with the flags `holon mail`, `holon feed` and `holon queue` take), the server's own
//! secrets, its storage settings and its policy file, and, for each folder, database, mail, feed
//! and queue, the deployments that use it and what a version waiting for a person would add
//! (editor/src/holdings.ts, here over `GET /admin/grants` and `GET /proposals`).

use ratatui::crossterm::event::KeyCode;
use serde_json::{json, Value};

use super::admin::{AdminAsk, AdminDo, Part};
use super::app::{App, Modal, Tone};
use super::flow::Row;
use crate::cmdline::BindArgs;
use crate::mcp::seg;

/// A resource flows reach by a name an admin binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Res {
    Folder,
    Database,
    Mail,
    Feed,
    Queue,
}

impl Res {
    /// Its routes' word: `/admin/{path}`.
    pub(crate) fn path(self) -> &'static str {
        match self {
            Res::Folder => "folders",
            Res::Database => "databases",
            Res::Mail => "mails",
            Res::Feed => "feeds",
            Res::Queue => "queues",
        }
    }

    /// Its command: `holon {command} add`.
    fn command(self) -> &'static str {
        match self {
            Res::Folder => "folder",
            Res::Database => "database",
            Res::Mail => "mail",
            Res::Feed => "feed",
            Res::Queue => "queue",
        }
    }

    pub(crate) fn of(p: Part) -> Option<Res> {
        match p {
            Part::Folders => Some(Res::Folder),
            Part::Databases => Some(Res::Database),
            Part::Mail => Some(Res::Mail),
            Part::Feeds => Some(Res::Feed),
            Part::Queues => Some(Res::Queue),
            _ => None,
        }
    }
}

/// What a dialog of the host parts asks for.
pub(crate) enum HostAsk {
    /// A mail, feed or queue bound, as its command's `add` takes it.
    Bind(Res),
    /// A server secret: its name, `--from` (none: its value is asked next), `--to`.
    SecretNew,
    /// A server secret set again: `--from`, or nothing to type its value next.
    SecretAgain(String),
    /// Its value, typed and never shown; where it may go, if said.
    SecretValue { name: String, to: Option<Vec<String>> },
    /// Where a server secret may go: hosts and databases, or anywhere.
    SecretTo(String),
    /// Which version a server secret goes back to.
    SecretBack(String),
    /// A storage setting's new value: its key.
    Storage(&'static str),
}

/// What a confirmed dialog of the host parts does.
pub(crate) enum HostDo {
    SecretBack { name: String, version: u64 },
    SecretRemove(String),
}

/// The storage settings, as `POST /admin/storage` takes them: the key, its row, what it is, an
/// example.
pub(crate) const STORAGE: [(&str, &str, &str, &str); 5] = [
    ("sweep_every", "Sweeps every", "How often the server sweeps the store of what nothing names: a while, or off.", "10m"),
    ("warn_at", "Warns at", "When a disk is full enough to warn of: how full, or how much is left free.", "85%"),
    ("full_at", "Full at", "When it is full enough to act on.", "95%"),
    ("when_full", "When full", "What it does then: warn, pause the deployments that write, or roll their oldest runs off.", "warn"),
    ("backup", "Backup", "Where the runs a log lets go of are written first: a folder's whole path, or none.", "none"),
];

/// The policy part's rows.
pub(crate) const POLICY_ROWS: [&str; 3] = ["holon.toml", "What flows get", "Held off"];

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("-").to_string()
}

fn field(k: &str, v: impl Into<String>) -> Row {
    Row::Field(k.to_string(), v.into(), Tone::Plain)
}

fn text(t: Tone, s: impl Into<String>) -> Row {
    Row::Text(t, s.into())
}

/// A grant item's kind and argument: `DbWrite("shop/orders")` is `DbWrite` of `shop/orders`;
/// `Clock` and `Limit(max_requests=80)` have none.
fn item_parts(item: &str) -> (&str, Option<String>) {
    let kind_end = item.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(item.len());
    let kind = &item[..kind_end];
    let arg = item[kind_end..].strip_prefix('(').and_then(|r| r.strip_suffix(')')).filter(|a| a.starts_with('"')).and_then(|a| serde_json::from_str::<String>(a).ok());
    (kind, arg)
}

/// One way something reaches a resource: how (`read`, `write`, …), a database's table or a
/// mail's recipient (`*`: any its binding allows), and whether the host took it back since.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Use {
    how: &'static str,
    table: Option<String>,
    revoked: bool,
}

/// What a grant item reaches, when it reaches a folder, a database, a mail, a feed or a queue:
/// which, by name, and how.
fn reaches(item: &str) -> Option<(Res, String, Use)> {
    let (kind, arg) = item_parts(item);
    let arg = arg?;
    let split = |arg: &str| match arg.split_once('/') {
        Some((name, table)) => (name.to_string(), Some(table.to_string())),
        None => (arg.to_string(), None),
    };
    let (res, how, (name, table)) = match kind {
        "FileRead" => (Res::Folder, "read", (arg, None)),
        "FileWrite" => (Res::Folder, "write", (arg, None)),
        "FileArrival" => (Res::Folder, "arrival", (arg, None)),
        "FileServe" => (Res::Folder, "serve", (arg, None)),
        "DbRead" => (Res::Database, "read", split(&arg)),
        "DbWrite" => (Res::Database, "write", split(&arg)),
        "MailArrival" => (Res::Mail, "receive", (arg, None)),
        "MailSend" => (Res::Mail, "send", split(&arg)),
        "FeedEvent" => (Res::Feed, "listen", (arg, None)),
        "QueueMessage" => (Res::Queue, "dequeue", (arg, None)),
        "QueueSend" => (Res::Queue, "enqueue", (arg, None)),
        _ => return None,
    };
    Some((res, name, Use { how, table, revoked: false }))
}

/// Whether deployment `slug` sees `binding` by its name: its own binding of the name, else the
/// server's.
fn sees(bindings: &[Value], binding: &Value, slug: &str) -> bool {
    let name = &binding["name"];
    let seen = bindings.iter().find(|b| b["name"] == *name && b["deployment"] == slug).or_else(|| bindings.iter().find(|b| b["name"] == *name && b["deployment"].is_null()));
    seen.is_some_and(|b| b["deployment"] == binding["deployment"])
}

/// The uses in `items` that reach `binding` (a `res`), for `slug`.
fn uses_in<'a>(res: Res, binding: &Value, bindings: &[Value], slug: &str, items: impl Iterator<Item = &'a Value>, revoked: bool) -> Vec<Use> {
    items
        .filter_map(Value::as_str)
        .filter_map(reaches)
        .filter(|(r, name, _)| *r == res && binding["name"] == name.as_str() && sees(bindings, binding, slug))
        .map(|(_, _, u)| Use { revoked, ..u })
        .collect()
}

/// The deployments that reach `binding`, and how, taken-back items among them.
pub(crate) fn holders_of(res: Res, binding: &Value, bindings: &[Value], held: &[Value]) -> Vec<(String, Vec<Use>)> {
    held.iter()
        .filter_map(|d| {
            let slug = d["slug"].as_str()?;
            let all = |k: &str| d[k].as_array().cloned().unwrap_or_default();
            let (items, revoked) = (all("items"), all("revoked"));
            let mut uses = uses_in(res, binding, bindings, slug, items.iter(), false);
            uses.extend(uses_in(res, binding, bindings, slug, revoked.iter(), true));
            (!uses.is_empty()).then(|| (slug.to_string(), uses))
        })
        .collect()
}

/// The versions waiting for a person that would reach `binding`, and how: what each asks that
/// is new.
pub(crate) fn waiting_for(res: Res, binding: &Value, bindings: &[Value], proposals: &[Value]) -> Vec<(String, Option<u64>, Vec<Use>)> {
    proposals
        .iter()
        .filter(|p| p["state"] == "waiting")
        .filter_map(|p| {
            let slug = p["slug"].as_str()?;
            let needs = p["needs"].as_array().cloned().unwrap_or_default();
            let uses = uses_in(res, binding, bindings, slug, needs.iter(), false);
            (!uses.is_empty()).then(|| (slug.to_string(), p["version"].as_u64(), uses))
        })
        .collect()
}

/// How something reaches a resource, as the page says it: `reads orders, refunds`, `writes
/// orders`, `starts on new files`, `mails ops@acme.com, addresses it computes`; each with
/// whether it was taken back.
pub(crate) fn uses_text(uses: &[Use]) -> Vec<(String, bool)> {
    let verbs = [
        ("read", "reads"),
        ("write", "writes"),
        ("arrival", "starts on new files"),
        ("serve", "serves its files"),
        ("send", "mails"),
        ("receive", "starts on mail it receives"),
        ("listen", "starts on each event it sends"),
        ("dequeue", "starts on each message it holds"),
        ("enqueue", "sends messages to it"),
    ];
    let mut out = vec![];
    for revoked in [false, true] {
        for (how, verb) in verbs {
            let these: Vec<&Use> = uses.iter().filter(|u| u.how == how && u.revoked == revoked).collect();
            if these.is_empty() {
                continue;
            }
            let mut named: Vec<String> = these.iter().filter_map(|u| u.table.clone()).collect();
            named.sort();
            named.dedup();
            // A mail's computed recipients, last.
            if how == "send" && named.iter().any(|t| t == "*") {
                named.retain(|t| t != "*");
                named.push("addresses it computes".into());
            }
            out.push((if named.is_empty() { verb.to_string() } else { format!("{verb} {}", named.join(", ")) }, revoked));
        }
    }
    out
}

/// The deployments whose grants read the secret `name`, and the waiting versions that would
/// where theirs doesn't now.
pub(crate) fn secret_readers(name: &str, held: &[Value], proposals: &[Value]) -> (Vec<String>, Vec<(String, Option<u64>)>) {
    let reads = |items: &Value| items.as_array().into_iter().flatten().filter_map(Value::as_str).any(|i| item_parts(i) == ("SecretRead", Some(name.to_string())));
    let now: Vec<String> = held.iter().filter(|d| reads(&d["items"])).filter_map(|d| d["slug"].as_str().map(String::from)).collect();
    let waiting = proposals.iter().filter(|p| p["state"] == "waiting" && reads(&p["needs"])).filter_map(|p| Some((p["slug"].as_str()?.to_string(), p["version"].as_u64()))).filter(|(slug, _)| !now.contains(slug)).collect();
    (now, waiting)
}

/// What a key of a host part takes, as can.rs's routes ask it.
pub(crate) fn takes(p: Part, k: KeyCode, row: usize, secrets: usize) -> &'static [&'static str] {
    match (p, k) {
        (Part::Mail | Part::Feeds | Part::Queues, KeyCode::Char('n')) => &["resource.bind"],
        (Part::Mail | Part::Feeds | Part::Queues, KeyCode::Char('D') | KeyCode::Delete) => &["resource.unbind"],
        (Part::Folders, KeyCode::Char('b')) => &["resource.bind"],
        (Part::Secrets, KeyCode::Char('n')) => &["resource.bind"],
        (Part::Secrets, KeyCode::Enter | KeyCode::Char('t' | 'R')) if row < secrets => &["resource.bind"],
        (Part::Secrets, KeyCode::Char('D') | KeyCode::Delete) if row < secrets => &["resource.unbind"],
        (Part::Storage, KeyCode::Enter) => &["host.operate"],
        (Part::Policy, KeyCode::Enter) if row == 0 => &["policy.edit"],
        _ => &[],
    }
}

impl App {
    /// The server's own secrets (`GET /admin/secrets`).
    fn server_secrets(&self) -> Vec<Value> {
        self.admin.secrets["secrets"].as_array().cloned().unwrap_or_default()
    }

    /// Each deployment's grant (`GET /admin/grants`), or why it can't be read.
    fn held(&self) -> Result<&[Value], &String> {
        match &self.admin.held_refused {
            Some(e) => Err(e),
            None => Ok(&self.admin.held),
        }
    }

    /// The bindings of `res`, as listed.
    pub(crate) fn bindings_of(&self, res: Res) -> &[Value] {
        let a = &self.admin;
        match res {
            Res::Folder => &a.folders,
            Res::Database => &a.databases,
            Res::Mail => &a.mails,
            Res::Feed => &a.feeds,
            Res::Queue => &a.queues,
        }
    }

    /// How many rows a host part has.
    pub(crate) fn host_part_rows(&self, p: Part) -> usize {
        match p {
            Part::Mail | Part::Feeds | Part::Queues => Res::of(p).map_or(0, |r| self.bindings_of(r).len()),
            // The server's secrets, then what its environment holds.
            Part::Secrets => self.server_secrets().len() + 1,
            Part::Storage => STORAGE.len(),
            Part::Policy => POLICY_ROWS.len(),
            _ => 0,
        }
    }

    /// Read the host parts' lists: each only by whoever may see it, else why not.
    pub(crate) fn host_part_load(&mut self) {
        let Some(c) = self.client.as_ref() else { return };
        let read = |path: &str, action: &str| match c.may(action, None) {
            Ok(()) => c.get(path),
            Err(why) => Err(why),
        };
        let list = |r: Result<Value, String>| r.map(|v| v.as_array().cloned().unwrap_or_default());
        let (mails, feeds, queues) = (list(read("/admin/mails", "resource.view")), list(read("/admin/feeds", "resource.view")), list(read("/admin/queues", "resource.view")));
        let secrets = read("/admin/secrets", "resource.view");
        let (held, host, policy) = (list(read("/admin/grants", "host.view")), read("/admin/host", "host.view"), read("/permissions", "policy.view"));
        let a = &mut self.admin;
        for (p, r) in [(Part::Mail, &mails), (Part::Feeds, &feeds), (Part::Queues, &queues)] {
            a.refused[p as usize] = r.as_ref().err().cloned();
        }
        a.refused[Part::Secrets as usize] = secrets.as_ref().err().cloned();
        a.refused[Part::Storage as usize] = host.as_ref().err().cloned();
        a.refused[Part::Policy as usize] = policy.as_ref().err().cloned();
        // Who uses what: none read, the resources say why.
        a.held_refused = held.as_ref().err().map(|e| format!("who uses it can't be shown here: {e}"));
        a.held = held.unwrap_or_default();
        (a.mails, a.feeds, a.queues) = (mails.unwrap_or_default(), feeds.unwrap_or_default(), queues.unwrap_or_default());
        a.secrets = secrets.unwrap_or(Value::Null);
        a.host = host.unwrap_or(Value::Null);
        a.policy = policy.unwrap_or(Value::Null);
    }

    /// The deployments that use `binding` (a `res`), and in amber what a waiting version would
    /// add.
    pub(crate) fn used_by(&self, res: Res, binding: &Value) -> Vec<Row> {
        let held = match self.held() {
            Ok(held) => held,
            Err(e) => return vec![text(Tone::Dim, e.clone())],
        };
        let bindings = self.bindings_of(res);
        let proposals = self.client.as_ref().map(|c| c.proposals.as_slice()).unwrap_or_default();
        let (holders, waiting) = (holders_of(res, binding, bindings, held), waiting_for(res, binding, bindings, proposals));
        let mut rows = vec![text(Tone::Head, format!("Used by ({})", holders.len()))];
        if holders.is_empty() && waiting.is_empty() {
            rows.push(text(Tone::Dim, "  no deployment uses it"));
        }
        for (slug, uses) in &holders {
            for (said, revoked) in uses_text(uses) {
                rows.push(if revoked { text(Tone::Dim, format!("  {slug}: {said} (taken back)")) } else { text(Tone::Plain, format!("  {slug}: {said}")) });
            }
        }
        for (slug, version, uses) in &waiting {
            let v = version.map(|v| format!(" v{v}")).unwrap_or_default();
            let said: Vec<String> = uses_text(uses).into_iter().map(|(s, _)| s).collect();
            rows.push(text(Tone::Warn, format!("  {slug}{v}, waiting for a person, would add: {}", said.join(", "))));
        }
        rows
    }

    /// The right pane for a host part's row `i`.
    pub(crate) fn host_part_pane(&self, p: Part, i: usize) -> Vec<Row> {
        let a = &self.admin;
        let Some(c) = self.client.as_ref() else { return vec![] };
        match p {
            Part::Mail | Part::Feeds | Part::Queues => {
                let Some(res) = Res::of(p) else { return vec![] };
                let Some(b) = self.bindings_of(res).get(i) else {
                    return vec![text(Tone::Dim, format!("n binds a {}, as holon {} add takes it.", res.command(), res.command()))];
                };
                let line = match res {
                    Res::Mail => crate::mails::line(b),
                    Res::Feed => crate::feeds::line(b),
                    _ => crate::queues::line(b),
                };
                let mut rows = vec![text(Tone::Head, s(&b["name"]))];
                // The line's parts, each on its own: they are two spaces apart.
                rows.extend(line.split("  ").skip(1).map(|part| text(Tone::Plain, part.trim().to_string())));
                rows.push(field("For", b["deployment"].as_str().map_or("every flow".into(), |d| format!("{d} only"))));
                rows.extend(self.used_by(res, b));
                rows.push(text(Tone::Dim, "del: unbind it · n: bind another"));
                rows
            }
            Part::Secrets => {
                let secrets = self.server_secrets();
                let Some(secret) = secrets.get(i) else {
                    let mut rows = vec![text(Tone::Head, "Read from the environment"), text(Tone::Dim, "A secret set as env:HOLON_SECRET_… reads one of these. Names only, never values.")];
                    match c.get("/admin/secrets/env") {
                        Ok(v) => rows.extend(crate::secretcmd::env_lines(&v).into_iter().map(|l| text(Tone::Plain, l))),
                        Err(e) => rows.push(text(Tone::Bad, e)),
                    }
                    return rows;
                };
                let name = s(&secret["name"]);
                let mut rows = vec![
                    text(Tone::Head, name.clone()),
                    field("Version", format!("v{}", secret["version"])),
                    field("Source", s(&secret["source"])),
                    field("Set", crate::cmdline::when(secret["set_at"].as_u64())),
                    field("May go", crate::secretcmd::may_go(&secret["to"]).trim_start_matches("may go ").to_string()),
                ];
                if let Some(problem) = secret["problem"].as_str() {
                    rows.push(Row::Field("Problem".into(), problem.into(), Tone::Bad));
                }
                match self.held() {
                    Ok(held) => {
                        let proposals = c.proposals.as_slice();
                        let (now, waiting) = secret_readers(&name, held, proposals);
                        rows.push(field("Read by", if now.is_empty() { "no deployment".into() } else { now.join(", ") }));
                        for (slug, version) in waiting {
                            rows.push(Row::Field(String::new(), format!("{slug}{}, waiting for a person, would read it", version.map(|v| format!(" v{v}")).unwrap_or_default()), Tone::Warn));
                        }
                    }
                    Err(e) => rows.push(text(Tone::Dim, e.clone())),
                }
                rows.push(text(Tone::Head, "History"));
                match c.get(&format!("/admin/secrets/{}", seg(&name))) {
                    Ok(v) => rows.extend(crate::secretcmd::history_lines(&v).into_iter().skip(1).map(|l| text(Tone::Plain, l))),
                    Err(e) => rows.push(text(Tone::Bad, e)),
                }
                rows.push(text(Tone::Dim, "Its value is never shown. enter: set it again · t: where it may go · R: roll back · del: remove"));
                rows
            }
            Part::Storage => {
                let Some((key, label, what, _)) = STORAGE.get(i) else { return vec![] };
                // No `backup` line: no one has chosen yet (permissions.rs `Backup`), which the host
                // warns of; its ask starts empty all the same.
                let now = match self.storage_value(key) {
                    v if *key == "backup" && v == "-" => "not chosen".to_string(),
                    v => v,
                };
                let mut rows = vec![text(Tone::Head, *label), text(Tone::Plain, *what), field("Now", now)];
                rows.push(text(Tone::Dim, "enter: change it (it is written into holon.toml's [storage], then read again)"));
                rows.push(text(Tone::Head, "The host"));
                rows.extend(crate::storage::host_lines(&a.host).into_iter().map(|l| text(Tone::Plain, l)));
                rows
            }
            Part::Policy => match i {
                0 => {
                    let mut rows = vec![text(Tone::Head, "holon.toml: what flows get without asking, and the most they may ask for")];
                    match a.policy["text"].as_str().filter(|t| !t.trim().is_empty()) {
                        Some(t) => rows.extend(t.lines().enumerate().map(|(n, l)| Row::Code { n: n as u32 + 1, text: l.to_string(), notes: vec![] })),
                        None => rows.push(text(Tone::Dim, "No policy file: the server's defaults hold.")),
                    }
                    rows.push(text(Tone::Dim, "enter: read holon.toml again and hold every deployment to it"));
                    rows
                }
                1 => {
                    let mut rows = vec![text(Tone::Head, "What flows get, and the limits: each one's default and ceiling")];
                    rows.extend(crate::grantcmd::policy_lines(&a.policy).into_iter().map(|l| text(Tone::Plain, l)));
                    rows
                }
                _ => {
                    let mut rows = vec![text(Tone::Head, "Deployments the policy holds off")];
                    match self.held() {
                        Ok(held) => {
                            let off: Vec<&Value> = held.iter().filter(|d| d["attention"].is_string()).collect();
                            if off.is_empty() {
                                rows.push(text(Tone::Plain, "None: the policy covers every deployment."));
                            }
                            rows.extend(off.iter().map(|d| Row::Field(s(&d["slug"]), s(&d["attention"]), Tone::Warn)));
                        }
                        Err(e) => rows.push(text(Tone::Dim, e.clone())),
                    }
                    rows
                }
            },
            _ => vec![],
        }
    }

    /// A storage setting as it is now: the policy's, else what the host says.
    pub(crate) fn storage_value(&self, key: &str) -> String {
        let a = &self.admin;
        let said = a.policy["storage"][key].as_str().map(String::from).or_else(|| match key {
            "backup" => a.host["backup"]["dir"].as_str().map(String::from),
            k => a.host[k].as_str().map(String::from),
        });
        said.unwrap_or_else(|| "-".into())
    }

    /// A key on a host part, its takes already allowed.
    pub(crate) fn host_part_key(&mut self, p: Part, k: KeyCode, i: usize) {
        let secrets = self.server_secrets();
        let secret = secrets.get(i).map(|s| s["name"].as_str().unwrap_or_default().to_string());
        match (p, k) {
            (Part::Mail, KeyCode::Char('n')) => self.ask(
                "Bind a mail",
                &[
                    "As holon mail add takes it: a name flows use, then --smtp SOURCE --from SENDER --to LIST to send,",
                    "--imap SOURCE [--folder F --done F --verdict-from SERVER --every T] to receive, --oauth SOURCE, --for SLUG",
                    "ops --smtp env:HOLON_SECRET_OPS_SMTP --from \"Treasury <treasury@acme.com>\" --to *@acme.com",
                ],
                "",
                AdminAsk::Host(HostAsk::Bind(Res::Mail)),
            ),
            (Part::Feeds, KeyCode::Char('n')) => self.ask(
                "Bind a feed",
                &[
                    "As holon feed add takes it: a name flows use, then --url SOURCE (or --database NAME --channel CHANNEL), and any of",
                    "--header NAME=SOURCE  --subscribe TEXT  --heartbeat TEXT --every T  --buffer N  --idle T  --for SLUG",
                    "prices --url env:HOLON_SECRET_PRICES_URL --header Authorization=env:HOLON_SECRET_PRICES_AUTH",
                ],
                "",
                AdminAsk::Host(HostAsk::Bind(Res::Feed)),
            ),
            (Part::Queues, KeyCode::Char('n')) => self.ask(
                "Bind a queue",
                &["As holon queue add takes it: a name flows use, then --url SOURCE, and any of --visibility T  --keep-failed  --for SLUG", "payout_events --url env:HOLON_SECRET_EVENTS_URL --keep-failed"],
                "",
                AdminAsk::Host(HostAsk::Bind(Res::Queue)),
            ),
            (Part::Mail | Part::Feeds | Part::Queues, KeyCode::Delete | KeyCode::Char('D')) => {
                let Some(res) = Res::of(p) else { return };
                if let Some(b) = self.bindings_of(res).get(i) {
                    let (name, deployment) = (s(&b["name"]), b["deployment"].as_str().map(String::from));
                    let line = match res {
                        Res::Mail => "Flows that use it can't reach it until it is bound again; its mail on the way is given up.",
                        Res::Feed => "Flows that use it can't reach it until it is bound again; its connections close.",
                        _ => "Flows that use it can't reach it until it is bound again; its listener stops.",
                    };
                    self.sure(format!("Unbind {name}?"), &[line], AdminDo::Unbind { res, name, deployment });
                }
            }
            (Part::Secrets, KeyCode::Char('n')) => self.ask(
                "A secret of the server's",
                &[
                    "Its name, then --from env:HOLON_SECRET_… or vault:<mount>/<path>#<field> (none: you type its value next),",
                    "and --to HOST,db:NAME for where it may go (none: wherever its flows' approvals show):",
                    "api_key --from env:HOLON_SECRET_API_KEY --to api.stripe.com",
                ],
                "",
                AdminAsk::Host(HostAsk::SecretNew),
            ),
            (Part::Secrets, KeyCode::Enter) => {
                if let Some(name) = secret {
                    self.ask(format!("Set {name} again"), &["--from env:HOLON_SECRET_… or vault:<mount>/<path>#<field>, or nothing to type its value next:"], "", AdminAsk::Host(HostAsk::SecretAgain(name)));
                }
            }
            (Part::Secrets, KeyCode::Char('t')) => {
                if let Some(name) = secret {
                    let now = secrets[i]["to"].as_array().map(|to| to.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")).unwrap_or_else(|| "anywhere".into());
                    let lines = ["Hosts and databases, with commas (api.stripe.com,db:shop), or anywhere: wherever its flows' approvals show."];
                    self.ask(format!("Where {name} may go"), &lines, now, AdminAsk::Host(HostAsk::SecretTo(name)));
                }
            }
            (Part::Secrets, KeyCode::Char('R')) => {
                if let Some(name) = secret {
                    let before = secrets[i]["version"].as_u64().unwrap_or(1).saturating_sub(1).max(1);
                    self.ask(format!("Roll {name} back"), &["To which version (its history lists them): what it would do is shown first."], before.to_string(), AdminAsk::Host(HostAsk::SecretBack(name)));
                }
            }
            (Part::Secrets, KeyCode::Delete | KeyCode::Char('D')) => {
                if let Some(name) = secret {
                    self.sure(format!("Remove the server's {name}?"), &["Every version goes. A flow that reads it and has none of its own can't run until it is set again."], AdminDo::Host(HostDo::SecretRemove(name)));
                }
            }
            (Part::Storage, KeyCode::Enter) => {
                if let Some((key, label, what, example)) = STORAGE.get(i) {
                    let lines = [*what, &format!("For example: {example}")];
                    self.ask(*label, &lines, self.storage_value(key).trim_matches('-').to_string(), AdminAsk::Host(HostAsk::Storage(key)));
                }
            }
            (Part::Policy, KeyCode::Enter) if i == 0 => self.sure("Read holon.toml again?", &["Every deployment is held to it as it is now: one that asks for more than it allows now pauses."], AdminDo::ReloadPolicy),
            _ => {}
        }
    }

    /// A dialog of a host part answered.
    pub(crate) fn host_part_answer(&mut self, ask: HostAsk, value: String) {
        let Some(c) = self.client.as_ref() else { return };
        match ask {
            HostAsk::Bind(res) => {
                let words = match crate::cmdline::split_words(&value) {
                    Ok(w) => w,
                    Err(e) => return self.fail(e),
                };
                let args: Vec<String> = std::iter::once("add".to_string()).chain(words).collect();
                let read: Result<BindArgs, String> = match res {
                    Res::Mail => crate::mails::read_args(&args),
                    Res::Feed => crate::feeds::read_args(&args),
                    _ => crate::queues::read_args(&args),
                };
                let read = match read {
                    Ok(r) if r.server.is_some() => return self.fail("--server isn't asked here: this is the server"),
                    Ok(r) => r,
                    Err(e) => return self.fail(e),
                };
                let [_, name] = read.words.as_slice() else { return self.fail(format!("one name, then the flags: as holon {} add takes them", res.command())) };
                let body = match res {
                    Res::Mail => crate::mails::add_body(name, &read),
                    Res::Feed => crate::feeds::add_body(name, &read),
                    _ => crate::queues::add_body(name, &read),
                };
                let body = match body {
                    Ok(b) => b,
                    Err(e) => return self.fail(e),
                };
                let r = c.post(&format!("/admin/{}", res.path()), body);
                self.admin_done(r, |v| match res {
                    Res::Mail => format!("bound {}", crate::mails::line(v)),
                    Res::Feed => format!("bound {}", crate::feeds::line(v)),
                    _ => format!("bound {}", crate::queues::line(v)),
                });
            }
            HostAsk::SecretNew | HostAsk::SecretAgain(_) => {
                let (words, flags) = match super::access::read_flags(&value, &["--from", "--to"]) {
                    Ok(r) => r,
                    Err(e) => return self.fail(e),
                };
                let name = match (&ask, words.as_slice()) {
                    (HostAsk::SecretAgain(name), []) => name.clone(),
                    (HostAsk::SecretNew, [name]) => name.clone(),
                    (HostAsk::SecretNew, _) => return self.fail("its name, one word: api_key"),
                    _ => return self.fail("--from, or nothing"),
                };
                let to = match flags.get("to").map(|t| crate::secretcmd::destinations(t)).transpose() {
                    Ok(to) => to,
                    Err(e) => return self.fail(e),
                };
                match flags.get("from") {
                    Some(from) if from.starts_with("env:") || from.starts_with("vault:") => {
                        let mut body = json!({ "from": from });
                        if let Some(to) = &to {
                            body["to"] = json!(to);
                        }
                        let r = c.post(&format!("/admin/secrets/{}", seg(&name)), body);
                        self.admin_done(r, |v| format!("{name}: version {} set for the server, read from {from}", v["version"]));
                    }
                    Some(from) => self.fail(format!("--from is env:HOLON_SECRET_NAME or vault:PATH#FIELD, not {from:?}")),
                    None => {
                        let lines = vec!["Typed here and sealed; never shown again, here or anywhere.".to_string()];
                        self.modal = Some(Modal::input(format!("{name}'s value"), lines, String::new(), true, super::app::Ask::Admin(AdminAsk::Host(HostAsk::SecretValue { name, to }))));
                    }
                }
            }
            HostAsk::SecretValue { name, to } => {
                let value = zeroize::Zeroizing::new(value);
                if value.is_empty() {
                    return self.fail("nothing was typed: it stays as it was");
                }
                let mut body = json!({ "value": value.as_str() });
                if let Some(to) = &to {
                    body["to"] = json!(to);
                }
                let r = c.post(&format!("/admin/secrets/{}", seg(&name)), body);
                self.admin_done(r, |v| format!("{name}: version {} set for the server", v["version"]));
            }
            HostAsk::SecretTo(name) => {
                let to = match value.trim() {
                    "anywhere" | "" => Value::Null,
                    list => match crate::secretcmd::destinations(list) {
                        Ok(to) => json!(to),
                        Err(e) => return self.fail(e),
                    },
                };
                let r = c.post(&format!("/admin/secrets/{}/cap", seg(&name)), json!({ "to": to }));
                self.admin_done(r, |v| format!("{name}: {}", crate::secretcmd::may_go(&v["to"])));
            }
            HostAsk::SecretBack(name) => {
                let Ok(version) = value.trim().trim_start_matches('v').parse::<u64>() else { return self.fail("a version: its number, as its history lists it") };
                match c.post(&format!("/admin/secrets/{}/rollback", seg(&name)), json!({ "version": version, "dry_run": true })) {
                    Ok(v) => {
                        // What it would do, without the command's word on running it again.
                        let preview = crate::secretcmd::rollback_preview(&name, &v);
                        let preview = preview.split(" Nothing changed").next().unwrap_or_default().to_string();
                        self.sure(format!("Roll {name} back to version {version}?"), &[preview.as_str()], AdminDo::Host(HostDo::SecretBack { name, version }));
                    }
                    Err(e) => self.fail(e),
                }
            }
            HostAsk::Storage(key) => {
                let mut body = serde_json::Map::new();
                body.insert(key.into(), value.trim().into());
                let r = c.post("/admin/storage", Value::Object(body));
                self.admin_done(r, |_| format!("{key} set to {}: holon.toml's [storage] is read again", value.trim()));
            }
        }
    }

    /// A confirmed dialog of a host part.
    pub(crate) fn host_part_do(&mut self, d: HostDo) {
        let Some(c) = self.client.as_ref() else { return };
        match d {
            HostDo::SecretBack { name, version } => {
                let r = c.post(&format!("/admin/secrets/{}/rollback", seg(&name)), json!({ "version": version, "dry_run": false }));
                self.admin_done(r, |v| format!("{name}: version {version} is current again, as version {}", v["version"]));
            }
            HostDo::SecretRemove(name) => {
                let r = c.delete(&format!("/admin/secrets/{}", seg(&name)));
                self.admin_done(r, |_| format!("{name}: the server's removed, every version"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_reaches_what_it_names_through_the_binding_its_flow_sees() {
        let r = |i: &str| reaches(i).map(|(res, name, u)| (res, name, u.how, u.table));
        assert_eq!(r("DbWrite(\"shop/orders\")"), Some((Res::Database, "shop".into(), "write", Some("orders".into()))));
        assert_eq!(r("FileArrival(\"inbox\")"), Some((Res::Folder, "inbox".into(), "arrival", None)));
        assert_eq!(r("MailSend(\"ops/*\")"), Some((Res::Mail, "ops".into(), "send", Some("*".into()))));
        assert_eq!(r("QueueSend(\"retries\")").map(|x| x.2), Some("enqueue"));
        assert_eq!(r("Clock"), None);
        assert_eq!(r("Limit(max_requests=80)"), None);
        // The server's binding, and one deployment's own of the same name: each sees its own.
        let (server, own) = (json!({ "name": "inbox", "deployment": null }), json!({ "name": "inbox", "deployment": "b" }));
        let bindings = [server.clone(), own.clone()];
        let held = [json!({ "slug": "a", "items": ["FileRead(\"inbox\")"], "revoked": ["FileWrite(\"inbox\")"] }), json!({ "slug": "b", "items": ["FileWrite(\"inbox\")"], "revoked": [] })];
        let holders = holders_of(Res::Folder, &server, &bindings, &held);
        assert_eq!(holders.len(), 1, "{holders:?}");
        assert_eq!(uses_text(&holders[0].1), [("reads".to_string(), false), ("writes".to_string(), true)]);
        assert_eq!(holders_of(Res::Folder, &own, &bindings, &held)[0].0, "b");
        let proposals = [json!({ "slug": "c", "version": 3, "state": "waiting", "needs": ["FileWrite(\"inbox\")"] }), json!({ "slug": "d", "state": "declined", "needs": ["FileWrite(\"inbox\")"] })];
        let waiting = waiting_for(Res::Folder, &server, &bindings, &proposals);
        assert_eq!(waiting.iter().map(|(s, v, _)| (s.as_str(), *v)).collect::<Vec<_>>(), [("c", Some(3))]);
        let mails = uses_text(&[Use { how: "send", table: Some("*".into()), revoked: false }, Use { how: "send", table: Some("ops@acme.com".into()), revoked: false }]);
        assert_eq!(mails[0].0, "mails ops@acme.com, addresses it computes");
    }

    #[test]
    fn a_secrets_readers_are_the_grants_that_read_it_and_the_versions_that_would() {
        let held = [json!({ "slug": "a", "items": ["SecretRead(\"api_key\")"] }), json!({ "slug": "b", "items": ["SecretRead(\"other\")"] })];
        let proposals = [json!({ "slug": "b", "version": 2, "state": "waiting", "needs": ["SecretRead(\"api_key\")"] }), json!({ "slug": "a", "version": 5, "state": "waiting", "needs": ["SecretRead(\"api_key\")"] })];
        let (now, waiting) = secret_readers("api_key", &held, &proposals);
        assert_eq!(now, ["a"]);
        assert_eq!(waiting, [("b".to_string(), Some(2))], "a reads it already");
    }
}
