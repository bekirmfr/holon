//! What a flow touches, as a dry run shows it (a deploy's preview, a restore's): each thing its
//! code may reach, by what it is (a folder, a database, a secret, a host it calls, a key it keeps,
//! the requests and clock that start it…), with what the change does to it (new, gone, as now),
//! whether this server is ready for it, and, for a restore, what the runs since did through it,
//! which a restore doesn't undo. The server works it out (`touches`), every client says it the
//! same way (`lines`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::folders::{self, FolderAccess};
use crate::permissions::Policy;
use crate::recorder::LogEntry;
use crate::secrets::SecretStore;

/// The groups a dry run shows, in order: what starts it, what it is given, what it reaches
/// outside, what it keeps, its limits.
const GROUPS: [(&str, &str); 6] =
    [("triggers", "What starts it"), ("resources", "Folders, databases and secrets"), ("outside", "What it reaches outside"), ("state", "What it keeps"), ("limits", "Its limits"), ("other", "Also")];

/// A capability item as a dry run groups it: its group, what kind of thing it touches, that
/// thing's name (empty for one of a kind: the requests it receives) and what it does with it.
pub(crate) fn part(item: &str) -> (&'static str, &'static str, String, String) {
    let (name, arg) = match item.split_once('(') {
        Some((name, rest)) => {
            let inner = rest.strip_suffix(')').unwrap_or(rest);
            (name, inner.strip_prefix('"').and_then(|i| i.strip_suffix('"')).unwrap_or(inner).to_string())
        }
        None => (item, String::new()),
    };
    let s = |s: &str| s.to_string();
    match name {
        "NetworkIngress" => ("triggers", "requests", arg, s("receive")),
        "Clock" => ("other", "clock", arg, s("read")),
        "Action" => ("triggers", "action", arg, s("by hand")),
        "WebSocket" => ("triggers", "websocket", arg, s("hold connections")),
        "AgentRuns" => ("triggers", "action", arg, s("by agents")),
        "Unrecorded" => ("other", "log", arg, s("not recorded")),
        "AgentReadsLogs" => ("other", "log", s("its lines"), s("read by agents")),
        "Await" => ("triggers", "wait", arg, s("wait for it")),
        "Approval" => match arg.split_once('/') {
            Some((name, "1")) => ("triggers", "approval", name.to_string(), s("a person approves")),
            Some((name, people)) => ("triggers", "approval", name.to_string(), format!("{people} people approve")),
            None => ("triggers", "approval", arg, s("people approve")),
        },
        "FileArrival" => ("resources", "folder", arg, s("watch")),
        "MailArrival" => ("resources", "mail", arg, s("receive")),
        "FeedEvent" => ("resources", "feed", arg, s("listen")),
        "FileRead" => ("resources", "folder", arg, s("read")),
        "FileWrite" => ("resources", "folder", arg, s("write")),
        "FileServe" => ("resources", "folder", arg, s("answer its callers with its files")),
        "DbRead" | "DbWrite" => {
            let verb = if name == "DbRead" { "read" } else { "write" };
            match arg.split_once('/') {
                Some((db, table)) => ("resources", "database", db.to_string(), format!("{verb} {table}")),
                None => ("resources", "database", arg, s(verb)),
            }
        }
        "SecretRead" => ("resources", "secret", arg, s("read")),
        "SecretAccess" => ("resources", "secret", arg, s("read any")),
        "SecretAnswer" => ("resources", "secret", arg, s("answer its callers with it")),
        "HttpRequest" => ("outside", "host", arg, s("call")),
        "PlainHttp" => ("outside", "host", arg, s("call over plain HTTP")),
        "NetworkEgress" => ("outside", "network", arg, s("send")),
        "PrivateAddresses" => ("outside", "network", s("private addresses"), s("reach")),
        "RunsAction" => match arg.split_once('/') {
            Some((flow, action)) => ("outside", "flow", flow.to_string(), format!("run {action}")),
            None => ("outside", "flow", arg, s("run")),
        },
        "ChannelSend" => ("outside", "channel", arg, s("send")),
        "ChannelRead" => ("outside", "channel", arg, s("let callers subscribe")),
        "MailSend" => match arg.split_once('/') {
            Some((mail, "*")) => ("outside", "mail", mail.to_string(), s("send to an address it computes")),
            Some((mail, to)) => ("outside", "mail", mail.to_string(), format!("send to {to}")),
            None => ("outside", "mail", arg, s("send")),
        },
        "QueueSend" => ("outside", "queue", arg, s("send")),
        "Spawn" => ("outside", "process", arg, s("start")),
        "Browser" => ("outside", "browser", arg, s("answer pages")),
        "StateRead" => ("state", "key", arg, s("read")),
        "StateWrite" => ("state", "key", arg, s("write")),
        "CollectionRead" => ("state", "collection", arg, s("read")),
        "CollectionWrite" => ("state", "collection", arg, s("put and delete")),
        "Limit" => match arg.split_once('=') {
            // Memory said in MiB, as the approval says it.
            Some((limit, n)) => match n.parse::<u64>() {
                Ok(bytes) if limit == holon_core::MEMORY_LIMIT => ("limits", "limit", limit.to_string(), format!("{} MiB", bytes / (1024 * 1024))),
                _ => ("limits", "limit", limit.to_string(), n.to_string()),
            },
            None => ("limits", "limit", arg, String::new()),
        },
        "Random" => ("other", "randomness", arg, s("use")),
        _ => ("other", "other", item.to_string(), String::new()),
    }
}

/// What a dry run asks of `touches`: the items granted now (none: a first deploy), the items the
/// code it would run asks, what of those needs a person's approval, what an agent's deploy
/// withholds, what it can run without, and where the code sends each secret it reads (core's
/// `sent_secrets`; empty when not worked out).
pub(crate) struct Ask<'a> {
    pub(crate) now: Option<&'a [String]>,
    pub(crate) then: &'a [String],
    pub(crate) needs: &'a [String],
    pub(crate) withheld: &'a [String],
    pub(crate) optional: &'a [String],
    pub(crate) sent: &'a BTreeMap<String, BTreeSet<String>>,
}

/// What a secret's read does, with where the code sends it, as the approval says it: "read, and
/// send it to https://hooks.slack.com; what is answered to it goes to https://api.example.com".
fn read_sending(name: &str, sent: &BTreeMap<String, BTreeSet<String>>) -> String {
    match sent.get(name).map(holon_author::describe::describe_sent_to) {
        Some((Some(own), Some(answers))) => format!("read, and send it to {own}; what is answered to it goes to {answers}"),
        Some((Some(own), None)) => format!("read, and send it to {own}"),
        Some((None, Some(answers))) => format!("read; what is answered to it goes to {answers}"),
        _ => "read".to_string(),
    }
}

/// What this server has for a flow: its secrets, folders and databases, its policy, and the flows
/// deployed beside it.
pub(crate) struct Server<'a> {
    pub(crate) slug: &'a str,
    pub(crate) secrets: &'a SecretStore,
    pub(crate) folders: &'a Arc<folders::Folders>,
    pub(crate) policy: &'a Policy,
    pub(crate) deployed: Vec<String>,
}

impl Server<'_> {
    /// Whether this server is ready for one thing the flow touches with `uses` (its items): why
    /// not, if it isn't, and what it has for it (a secret's version and where it comes from).
    fn ready(&self, kind: &str, name: &str, uses: &[(String, String)], ask: &Ask) -> (Option<String>, Option<String>) {
        let optional = uses.iter().all(|(item, _)| ask.optional.contains(item));
        let writes = uses.iter().any(|(item, _)| item.starts_with("FileWrite(") || item.starts_with("DbWrite("));
        let refused = uses.iter().find_map(|(item, _)| self.policy.refuses_item(item));
        if let Some(why) = refused {
            return (Some(format!("this server refuses it: {why}")), None);
        }
        match kind {
            "secret" if !name.is_empty() => {
                if uses.iter().any(|(item, _)| ask.withheld.contains(item)) {
                    return (Some("withheld: an agent's deploy leaves it out, for a person to grant back".into()), None);
                }
                match self.secrets.list(self.slug).into_iter().find(|s| s.name == name) {
                    None if optional => (Some("not set: it runs without it".into()), None),
                    None => (Some("not set: the steps that read it get nothing until it is".into()), None),
                    Some(s) => match s.problem {
                        Some(p) => (Some(format!("v{} has no value: {p}", s.version)), None),
                        None => (None, Some(format!("set, v{} ({})", s.version, if s.source == "local" { "a value kept here" } else { &s.source }))),
                    },
                }
            }
            "folder" => match folders::Scoped::new(Arc::clone(self.folders), self.slug).get(name) {
                None if optional => (Some("not bound on this server: it runs without it".into()), None),
                None => (Some("not bound on this server".into()), None),
                Some(f) if writes && f.access != FolderAccess::ReadWrite => (Some("bound read-only, and it writes there".into()), None),
                Some(f) => (None, Some(if f.access == FolderAccess::ReadWrite { "bound".into() } else { "bound read-only".into() })),
            },
            "database" => match self.folders.databases.resolve(self.slug, name) {
                None if optional => (Some("not bound on this server: it runs without it".into()), None),
                None => (Some("not bound on this server".into()), None),
                Some(d) if writes && d.access != FolderAccess::ReadWrite => (Some("bound read-only, and it writes there".into()), None),
                Some(d) => (None, Some(format!("bound ({})", d.engine))),
            },
            "flow" if !self.deployed.iter().any(|s| s == name) => (Some("not deployed on this server".into()), None),
            _ => (None, None),
        }
    }
}

/// What a flow touches, as a dry run shows it: one row per thing (a folder, a host, a key…), in
/// the order of [`GROUPS`], each with what it does with it (`uses`), what the change does to it
/// (`change`: `new`, `gone`, `same`, or `changed` when only some of its uses are new or gone),
/// whether this server is ready for it (`ready`: why not), and what the server has for it.
pub(crate) fn touches(server: &Server, ask: &Ask) -> Value {
    let mut items: Vec<&String> = ask.then.iter().collect();
    for i in ask.now.into_iter().flatten() {
        if !items.contains(&i) {
            items.push(i);
        }
    }
    // By its group's place, its kind and its name: its items, each with what it does.
    type Thing = (usize, &'static str, String);
    let mut rows: BTreeMap<Thing, Vec<(String, String)>> = BTreeMap::new();
    for item in items {
        let (group, kind, name, what) = part(item);
        // A secret's read says where the code sends it, and comes first: what else is done with
        // the secret follows.
        let read = item.starts_with("SecretRead(");
        let what = if read { read_sending(&name, ask.sent) } else { what };
        let at = GROUPS.iter().position(|(g, _)| *g == group).unwrap_or(GROUPS.len());
        let uses = rows.entry((at, kind, name)).or_default();
        uses.insert(if read { 0 } else { uses.len() }, (item.clone(), what));
    }
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|((at, kind, name), uses)| {
            // A first deploy (nothing granted now): all of it is new.
            let change = |item: &String| match (ask.now.is_some_and(|n| n.contains(item)), ask.then.contains(item)) {
                (true, true) => "same",
                (false, true) => "new",
                (_, false) => "gone",
            };
            let changes: Vec<&str> = uses.iter().map(|(item, _)| change(item)).collect();
            let row_change = if changes.iter().all(|c| *c == changes[0]) { changes[0] } else { "changed" };
            let kept: Vec<(String, String)> = uses.iter().filter(|(item, _)| ask.then.contains(item)).cloned().collect();
            let (ready, has) = if kept.is_empty() { (None, None) } else { server.ready(kind, &name, &kept, ask) };
            json!({
                "group": GROUPS.get(at).map_or("other", |(g, _)| *g),
                "kind": kind,
                "name": name,
                "change": row_change,
                "uses": uses.iter().zip(&changes).map(|((item, what), change)| json!({
                    "item": item,
                    "what": what,
                    "words": holon_author::agent::describe(item),
                    "change": change,
                    "approval": ask.needs.contains(item),
                    "optional": ask.optional.contains(item),
                })).collect::<Vec<_>>(),
                "ready": ready,
                "has": has,
            })
        })
        .collect();
    Value::Array(rows)
}

/// Add to `touches` what a restore does to each thing: the effects the runs `since` performed
/// through it (`since`: how many), which a restore doesn't undo, and, for a key or a collection,
/// what putting the state back does to it (`restores`: `changes`, `brings back`, `drops`, or
/// `keeps` as it is), from the state's dry run (`state`).
pub(crate) fn with_restore(touches: &mut Value, since: &[LogEntry], state: &Value) {
    let mut performed: BTreeMap<&str, u64> = BTreeMap::new();
    for p in since.iter().flat_map(|e| &e.performed) {
        *performed.entry(p.capability.as_str()).or_default() += 1;
    }
    let listed = |k: &str, key: &str| state[k].as_array().is_some_and(|a| a.iter().any(|v| v.as_str() == Some(key)));
    for row in touches.as_array_mut().into_iter().flatten() {
        let n: u64 = row["uses"].as_array().into_iter().flatten().filter_map(|u| u["item"].as_str()).filter(|i| !i.starts_with("StateWrite(") && !i.starts_with("CollectionWrite(")).map(|i| performed.get(i).copied().unwrap_or(0)).sum();
        if n > 0 {
            row["since"] = json!(n);
        }
        let key = match row["kind"].as_str() {
            Some("key") => row["name"].as_str().unwrap_or_default().to_string(),
            Some("collection") => format!("#collection:{}", row["name"].as_str().unwrap_or_default()),
            _ => continue,
        };
        if !state.is_object() {
            continue;
        }
        row["restores"] = json!(if listed("changed", &key) {
            "changes"
        } else if listed("added", &key) {
            "brings back"
        } else if listed("removed", &key) {
            "drops"
        } else {
            "keeps"
        });
    }
}

/// The deploy settings that differ between the version deployed now (`now`) and the one a dry run
/// would deploy (`then`): the sites whose pages it answers.
pub(crate) fn settings(now: &[String], then: &[String]) -> Value {
    let sites = |b: &[String]| if b.is_empty() { "none".to_string() } else { b.join(" ") };
    let mut out = vec![];
    if now != then {
        out.push(json!({ "name": "answers pages from", "now": sites(now), "then": sites(then) }));
    }
    Value::Array(out)
}

/// A line of a dry run, as a client marks it: a heading, something new (`approve`: a person
/// approves it by confirming), gone, as now, changed, or something the server isn't ready for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Mark {
    Head,
    New,
    Approve,
    Gone,
    Same,
    Changed,
    Unready,
}

impl Mark {
    /// Its sign at the start of a line, for a client without colour.
    pub(crate) fn sign(self) -> &'static str {
        match self {
            Mark::Head => "",
            Mark::New | Mark::Approve => "+ ",
            Mark::Gone => "- ",
            Mark::Same => "· ",
            Mark::Changed => "~ ",
            Mark::Unready => "! ",
        }
    }
}

/// A line of a dry run: its mark, what it says, and a note on it (why the server isn't ready, what
/// a restore does to it), which a client puts after it or under it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Said {
    pub(crate) mark: Mark,
    pub(crate) text: String,
    pub(crate) note: String,
}

/// `touches` and `settings` as a dry run's lines, group by group: a row each, marked with what
/// the change does to it, its uses (those that change marked), and a note: what the server has
/// for it or why it isn't ready, and for a restore, what it does to a key and what the runs since
/// did through it.
pub(crate) fn lines(touches: &Value, settings: &Value) -> Vec<Said> {
    let said = |mark, text: String| Said { mark, text, note: String::new() };
    let mut out = vec![];
    let rows = touches.as_array().map(Vec::as_slice).unwrap_or_default();
    for (group, heading) in GROUPS {
        let in_group: Vec<&Value> = rows.iter().filter(|r| r["group"] == group).collect();
        if in_group.is_empty() {
            continue;
        }
        out.push(said(Mark::Head, heading.to_string()));
        for row in in_group {
            out.push(row_line(row));
        }
    }
    let settings = settings.as_array().map(Vec::as_slice).unwrap_or_default();
    if !settings.is_empty() {
        out.push(said(Mark::Head, "Its settings".to_string()));
        for s in settings {
            out.push(said(Mark::Changed, format!("{}: {} → {}", s["name"].as_str().unwrap_or_default(), s["now"].as_str().unwrap_or_default(), s["then"].as_str().unwrap_or_default())));
        }
    }
    out
}

/// One thing a flow touches, as a line.
fn row_line(row: &Value) -> Said {
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let (kind, name) = (s(&row["kind"]), s(&row["name"]));
    let uses: Vec<&Value> = row["uses"].as_array().into_iter().flatten().collect();
    let approval = uses.iter().any(|u| u["approval"] == true);
    let mut mark = match row["change"].as_str() {
        Some("new") if approval => Mark::Approve,
        Some("new") => Mark::New,
        Some("gone") => Mark::Gone,
        Some("changed") => Mark::Changed,
        _ => Mark::Same,
    };
    // A use is marked when it changes and the row as a whole doesn't say so.
    let said: Vec<String> = uses
        .iter()
        .filter(|u| !u["what"].as_str().unwrap_or_default().is_empty())
        .map(|u| {
            let what = s(&u["what"]);
            let sign = match u["change"].as_str() {
                Some("new") if mark == Mark::Changed => "+",
                Some("gone") if mark == Mark::Changed => "−",
                _ => "",
            };
            let optional = if u["optional"] == true { " (optional)" } else { "" };
            format!("{sign}{what}{optional}")
        })
        .collect();
    let thing = match (kind.as_str(), name.is_empty()) {
        ("requests", _) => "requests".to_string(),
        ("clock", _) => "the clock".to_string(),
        ("process", _) => "processes".to_string(),
        ("randomness", _) => "randomness".to_string(),
        ("network", true) => "the network".to_string(),
        ("other", _) | ("network", false) => name.clone(),
        ("limit", _) => name.clone(),
        (_, true) => format!("{kind}s"),
        _ => format!("{kind} {name}"),
    };
    let line = if kind == "limit" { format!("{thing} = {}", said.join(", ")) } else if said.is_empty() { thing } else { format!("{thing}: {}", said.join(", ")) };
    let mut notes = vec![];
    if mark == Mark::Approve {
        notes.push("approved by confirming".to_string());
    } else if mark == Mark::New {
        notes.push("new".to_string());
    } else if mark == Mark::Changed && approval {
        notes.push("the new use approved by confirming".to_string());
    }
    if let Some(why) = row["ready"].as_str() {
        notes.push(why.to_string());
        if mark != Mark::Gone {
            mark = Mark::Unready;
        }
    } else if let Some(has) = row["has"].as_str() {
        notes.push(has.to_string());
    }
    if mark == Mark::Gone {
        notes.push("no longer".to_string());
    }
    if let Some(r) = row["restores"].as_str() {
        notes.push(match r {
            "keeps" => "its value stays as it is".to_string(),
            other => format!("putting the state back {other} it"),
        });
    }
    if let Some(n) = row["since"].as_u64() {
        notes.push(format!("done {n} time{} by the runs since, not undone", if n == 1 { "" } else { "s" }));
    }
    Said { mark, text: line, note: notes.join("; ") }
}

/// Dry-run lines as plain text, for a terminal without colour or an agent: a heading as it is, a
/// row after its sign, with its note after a dash.
pub(crate) fn text(lines: &[Said]) -> Vec<String> {
    lines
        .iter()
        .map(|l| match (l.mark, l.note.is_empty()) {
            (Mark::Head, _) => l.text.clone(),
            (_, true) => format!("  {}{}", l.mark.sign(), l.text),
            (_, false) => format!("  {}{} — {}", l.mark.sign(), l.text, l.note),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capability_is_grouped_by_the_thing_it_touches() {
        assert_eq!(part("FileWrite(\"inbox\")"), ("resources", "folder", "inbox".into(), "write".into()));
        assert_eq!(part("DbRead(\"shop/orders\")"), ("resources", "database", "shop".into(), "read orders".into()));
        assert_eq!(part("HttpRequest(\"hooks.slack.com\")"), ("outside", "host", "hooks.slack.com".into(), "call".into()));
        assert_eq!(part("RunsAction(\"billing/charge\")"), ("outside", "flow", "billing".into(), "run charge".into()));
        assert_eq!(part("CollectionWrite(\"notes\")"), ("state", "collection", "notes".into(), "put and delete".into()));
        assert_eq!(part("Limit(fuel=5000)"), ("limits", "limit", "fuel".into(), "5000".into()));
        assert_eq!(part("NetworkIngress"), ("triggers", "requests", String::new(), "receive".into()));
        assert_eq!(part("SecretAnswer(\"maps_key\")"), ("resources", "secret", "maps_key".into(), "answer its callers with it".into()));
    }

    /// A secret's row says where the code sends it, and what is answered to it, read first.
    #[test]
    fn a_secret_s_row_says_where_it_goes() {
        let store = SecretStore::memory();
        let folders = Arc::new(folders::Folders::memory());
        let policy = Policy::default();
        let server = Server { slug: "f", secrets: &store, folders: &folders, policy: &policy, deployed: vec![] };
        let s = |v: &[&str]| v.iter().map(|i| i.to_string()).collect::<Vec<_>>();
        let then = s(&["SecretAnswer(\"maps_key\")", "SecretRead(\"maps_key\")", "SecretRead(\"client_secret\")", "SecretRead(\"license_key\")"]);
        let to = |d: &[&str]| d.iter().map(|d| d.to_string()).collect::<BTreeSet<_>>();
        let sent = BTreeMap::from([("client_secret".to_string(), to(&["auth.example", "answer:api.example"])), ("license_key".to_string(), to(&["db:vendor"]))]);
        let rows = touches(&server, &Ask { now: Some(&then), then: &then, needs: &[], withheld: &[], optional: &[], sent: &sent });
        let said = text(&lines(&rows, &json!([])));
        let row = |name: &str| said.iter().find(|l| l.contains(&format!("secret {name}:"))).cloned().unwrap_or_else(|| panic!("{name} in {said:#?}"));
        assert!(row("client_secret").contains("secret client_secret: read, and send it to https://auth.example; what is answered to it goes to https://api.example"), "{said:#?}");
        assert!(row("license_key").contains("secret license_key: read, and send it to the database \"vendor\""), "{said:#?}");
        assert!(row("maps_key").contains("secret maps_key: read, answer its callers with it"), "{said:#?}");
        assert!(!said.iter().any(|l| l.contains("SecretAnswer")), "said in words, not as the item: {said:#?}");
    }

    #[test]
    fn a_restore_s_rows_say_what_changes_what_is_not_ready_and_what_is_not_undone() {
        let store = SecretStore::memory();
        store.set("f", "api_key", "a-dummy-value-for-tests", None).unwrap();
        let folders = Arc::new(folders::Folders::memory());
        let policy = Policy::default();
        let server = Server { slug: "f", secrets: &store, folders: &folders, policy: &policy, deployed: vec![] };
        let s = |v: &[&str]| v.iter().map(|i| i.to_string()).collect::<Vec<_>>();
        let now = s(&["NetworkIngress", "StateWrite(\"count\")", "HttpRequest(\"a.example\")", "SecretRead(\"api_key\")", "FileRead(\"inbox\")"]);
        let then = s(&["NetworkIngress", "StateWrite(\"count\")", "HttpRequest(\"b.example\")", "SecretRead(\"api_key\")", "SecretRead(\"other\")", "FileRead(\"inbox\")", "FileWrite(\"inbox\")"]);
        let needs = s(&["HttpRequest(\"b.example\")", "FileWrite(\"inbox\")"]);
        let mut rows = touches(&server, &Ask { now: Some(&now), then: &then, needs: &needs, withheld: &[], optional: &[], sent: &BTreeMap::new() });
        // Two runs since the point: one called b.example and wrote the key, which isn't counted
        // (putting the state back is what a restore does to it); the other called it again.
        let ran = |caps: &[&str]| LogEntry {
            performed: caps.iter().map(|c| crate::PerformedEntry { capability: c.to_string(), payload: crate::SValue::from_core(&holon_core::Value::Boolean(true)) }).collect(),
            ..LogEntry::default()
        };
        let since = [ran(&["HttpRequest(\"b.example\")", "StateWrite(\"count\")"]), ran(&["HttpRequest(\"b.example\")"])];
        with_restore(&mut rows, &since, &json!({ "changed": ["count"], "added": [], "removed": [] }));
        let said = text(&lines(&rows, &settings(&[], &["https://shop.example".to_string()])));
        assert_eq!(
            said,
            [
                "What starts it",
                "  · requests: receive",
                "Folders, databases and secrets",
                "  ! folder inbox: read, +write — the new use approved by confirming; not bound on this server",
                "  · secret api_key: read — set, v1 (a value kept here)",
                "  ! secret other: read — new; not set: the steps that read it get nothing until it is",
                "What it reaches outside",
                "  - host a.example: call — no longer",
                "  + host b.example: call — approved by confirming; done 2 times by the runs since, not undone",
                "What it keeps",
                "  · key count: write — putting the state back changes it",
                "Its settings",
                "  ~ answers pages from: none → https://shop.example",
            ]
        );
    }
}
