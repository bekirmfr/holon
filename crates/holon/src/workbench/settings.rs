//! A deployment's Settings section, as the web console's flow Settings (editor/src/FlowSettings.tsx):
//! its limits and the runs it keeps (lowered at once; raised by a new version), the plain-HTTP
//! hosts and private addresses it was granted (each revoked here), whether it keeps running when
//! something it requires is revoked, whether agents propose its versions and whether theirs deploy
//! at once, its waiting runs, its sessions (a stopped flow runs again from one), and removing it,
//! its keys forgotten too.

use serde_json::{json, Value};

use super::app::{App, Ask, Modal, Then, Tone};
use super::client::Detail;
use super::flow::Row;
use crate::mcp::seg;

/// The limits Settings shows by name, in words and their unit, whether asked for or not (the web's
/// `SETTING_LIMITS`, flowFacts.ts); any other the flow asked for after them.
const LIMITS: [(&str, &str, &str); 5] = [
    ("max_requests", "calls out per run", ""),
    ("run_budget_ms", "run budget", "ms"),
    ("max_request_bytes", "request body size", "bytes"),
    ("keep_runs", "newest runs kept", "runs"),
    ("keep_run_days", "none older than", "days"),
];

/// A row of the Settings section.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Setting {
    Head(&'static str),
    /// A limit as its runs are held to it (the web's `LimitRow`): what holds now, what it asked
    /// for, what it was granted (asked, else the host's default), what it was lowered to, and the
    /// policy's ceiling.
    Limit { name: String, label: String, unit: String, now: u64, asked: Option<u64>, base: u64, lowered: Option<u64>, ceiling: Option<u64> },
    /// A host it asked to reach over plain HTTP; whether that is granted now.
    PlainHttp { host: String, held: bool },
    /// Whether it may reach private addresses, which it asked for.
    PrivateAddresses { held: bool },
    KeepRunning(bool),
    AgentsPropose(bool),
    AgentsAtOnce(bool),
    /// Its runs waiting for something, by how many.
    Waiting(u64),
    /// A session: its number, what it started from, when it ended (none while it runs), whether it
    /// kept state to run again from.
    Session { number: u64, from: String, ended: Option<u64>, state: bool },
    /// Remove it; forgetting its keys too.
    Remove { forget: bool },
}

/// `n` in its unit: a run budget in seconds where whole, a size as people read it.
fn said(n: u64, unit: &str) -> String {
    match unit {
        "bytes" => crate::folders::size_text(n),
        "ms" if n >= 1000 && n.is_multiple_of(1000) => format!("{} s", n / 1000),
        "" => n.to_string(),
        u => format!("{n} {u}"),
    }
}

/// The limit rows for `d`'s grant under the host's policy (`GET /permissions`, null when it can't
/// be read: then none): the named ones first, then any other it asked for; a limit the host doesn't
/// know is left out (the web's `limitRows`).
fn limit_rows(grant: &Value, policy: &Value) -> Vec<Setting> {
    let asked = &grant["requirements"]["limits"];
    let items: Vec<&str> = grant["items"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let mut named: Vec<(String, String, String)> = LIMITS.iter().map(|(n, l, u)| (n.to_string(), l.to_string(), u.to_string())).collect();
    let mut extra: Vec<&String> = asked.as_object().into_iter().flat_map(|m| m.keys()).filter(|k| !LIMITS.iter().any(|(n, ..)| n == k)).collect();
    extra.sort();
    named.extend(extra.into_iter().map(|k| (k.clone(), k.clone(), if k.ends_with("_ms") { "ms" } else if k.ends_with("_bytes") { "bytes" } else { "" }.to_string())));
    named
        .into_iter()
        .filter_map(|(name, label, unit)| {
            let default = policy["defaults"][&name].as_u64()?;
            let a = asked[&name].as_u64();
            let granted = a.is_some_and(|a| items.contains(&format!("Limit({name}={a})").as_str()));
            let base = if granted { a.unwrap_or(default) } else { default };
            let lowered = grant["lowered"][&name].as_u64();
            let ceiling = policy["ceilings"][&name].as_u64();
            let now = [Some(base), lowered, ceiling].into_iter().flatten().min().unwrap_or(base);
            Some(Setting::Limit { name, label, unit, now, asked: a, base, lowered, ceiling })
        })
        .collect()
}

/// The Settings section's rows for `d`, in groups.
pub(crate) fn settings_rows(d: &Detail) -> Vec<Setting> {
    let grant = &d.grant["grant"];
    let mut rows = vec![Setting::Head("Limits and runs kept")];
    rows.extend(limit_rows(grant, &d.policy));
    let items: Vec<&str> = grant["items"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let plain: Vec<&str> = grant["requirements"]["plain_http"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let private = grant["requirements"]["private_addresses"] == true;
    if !plain.is_empty() || private {
        rows.push(Setting::Head("Network"));
        rows.extend(plain.into_iter().map(|h| Setting::PlainHttp { host: h.to_string(), held: items.contains(&format!("PlainHttp({h:?})").as_str()) }));
        if private {
            rows.push(Setting::PrivateAddresses { held: items.contains(&"PrivateAddresses") });
        }
    }
    if !grant.is_null() {
        rows.push(Setting::Head("When something it requires is revoked"));
        rows.push(Setting::KeepRunning(grant["keep_running"] == true));
    }
    rows.push(Setting::Head("Agents"));
    rows.push(Setting::AgentsPropose(d.entry["agents_propose"] != false));
    rows.push(Setting::AgentsAtOnce(d.entry["agents_deploy_at_once"] == true));
    let waiting = d.entry["waiting"].as_u64().unwrap_or(0);
    if waiting > 0 {
        rows.push(Setting::Head("Waiting runs"));
        rows.push(Setting::Waiting(waiting));
    }
    if !d.sessions.is_empty() {
        rows.push(Setting::Head("Sessions, newest first"));
        rows.extend(d.sessions.iter().rev().map(|s| Setting::Session {
            number: s["number"].as_u64().unwrap_or(0),
            from: s["from"].as_str().unwrap_or_default().to_string(),
            ended: s["ended_at"].as_u64(),
            state: s.get("state").is_some_and(|v| v.as_object().is_some_and(|m| !m.is_empty())),
        }));
    }
    rows.push(Setting::Head("Remove it"));
    rows.push(Setting::Remove { forget: false });
    rows.push(Setting::Remove { forget: true });
    rows
}

/// A row as the list says it: what it is, and where it stands.
pub(crate) fn setting_line(s: &Setting, status: &str) -> (Tone, String, String) {
    let on = |b: bool| if b { "on" } else { "off" };
    match s {
        Setting::Head(h) => (Tone::Head, h.to_string(), String::new()),
        Setting::Limit { label, unit, now, lowered, base, asked, ceiling, .. } => {
            let from = if lowered.is_some_and(|l| l <= *base) {
                format!("lowered from {}", said(*base, unit))
            } else if asked.is_some_and(|a| a == *base) {
                "asked for".into()
            } else {
                "the host's default".into()
            };
            let held = if ceiling.is_some_and(|c| c < (*base).min(lowered.unwrap_or(u64::MAX))) { ", held to the policy's ceiling" } else { "" };
            (Tone::Plain, format!("  {label}"), format!("{}  ({from}{held})", said(*now, unit)))
        }
        Setting::PlainHttp { host, held } => (if *held { Tone::Plain } else { Tone::Dim }, format!("  plain HTTP to {host}"), if *held { "granted".into() } else { "revoked".into() }),
        Setting::PrivateAddresses { held } => (if *held { Tone::Plain } else { Tone::Dim }, "  private addresses".into(), if *held { "may reach them".into() } else { "revoked".into() }),
        Setting::KeepRunning(b) => (Tone::Plain, "  keep running".into(), if *b { "on: it keeps running, and a run that needs it fails".into() } else { "off: it pauses until a person decides".into() }),
        Setting::AgentsPropose(b) => (Tone::Plain, "  agents may propose versions".into(), on(*b).into()),
        Setting::AgentsAtOnce(b) => (if *b { Tone::Warn } else { Tone::Plain }, "  an agent's version deploys at once".into(), if *b { "on: one that asks for nothing more deploys without a person".into() } else { "off: each waits for a person".into() }),
        Setting::Waiting(n) => (Tone::Warn, format!("  {n} run{} waiting", if *n == 1 { "" } else { "s" }), "enter: Approvals › Runs waiting".into()),
        Setting::Session { number, from, ended, state } => {
            let when = match ended {
                Some(t) => format!("ended {}", crate::cmdline::when(Some(*t))),
                None => "running now".into(),
            };
            let again = if *state && ended.is_some() && status == "stopped" { "  · enter runs from it" } else { "" };
            (Tone::Plain, format!("  session {number}"), format!("from {from}, {when}{again}"))
        }
        Setting::Remove { forget: false } => (Tone::Bad, "  remove it".into(), "its runs, history and state stay on record".into()),
        Setting::Remove { forget: true } => (Tone::Bad, "  remove it, and forget its keys".into(), "what was sealed under them can't be read again".into()),
    }
}

/// The right pane for a row: what it is, in full, and what enter does.
pub(crate) fn settings_pane(d: &Detail, s: &Setting) -> Vec<Row> {
    let t = |tone, s: &str| Row::Text(tone, s.to_string());
    let field = |k: &str, v: String| Row::Field(k.to_string(), v, Tone::Plain);
    match s {
        Setting::Head(h) => vec![t(Tone::Head, h)],
        Setting::Limit { name, label, unit, now, asked, base, lowered, ceiling } => {
            let mut rows = vec![t(Tone::Head, label), field("Holds now", said(*now, unit)), field("Name", name.clone())];
            rows.push(field("Asked for", asked.map_or("nothing: the host's default".into(), |a| said(a, unit))));
            rows.push(field("Granted", said(*base, unit)));
            if let Some(l) = lowered {
                rows.push(field("Lowered to", said(*l, unit)));
            }
            if let Some(c) = ceiling {
                rows.push(field("Ceiling", said(*c, unit)));
            }
            rows.push(t(Tone::Dim, "enter: lower it now, without a new version: the next runs are held to it. Raising it takes a new version, which a person approves."));
            rows
        }
        Setting::PlainHttp { host, held } => vec![
            t(Tone::Head, &format!("Plain HTTP to {host}")),
            t(Tone::Plain, if *held { "Its calls to it go unencrypted: whoever is on the way can read and change them." } else { "Revoked: a call to it over plain HTTP fails." }),
            t(Tone::Dim, if *held { "enter: revoke it" } else { "Granting it back is in Access." }),
        ],
        Setting::PrivateAddresses { held } => vec![
            t(Tone::Head, "Private addresses"),
            t(Tone::Plain, if *held { "It may reach addresses on this host's own network (10.x, 192.168.x, localhost)." } else { "Revoked: it reaches no private address." }),
            t(Tone::Dim, if *held { "enter: revoke it" } else { "Granting it back is in Access." }),
        ],
        Setting::KeepRunning(b) => vec![
            t(Tone::Head, "When something it requires is revoked"),
            t(Tone::Plain, if *b { "It keeps running: a run that needs what was revoked fails." } else { "It pauses, and waits for a person to decide." }),
            t(Tone::Dim, "enter: turn it the other way (flow.approve-deploy)"),
        ],
        Setting::AgentsPropose(b) => vec![
            t(Tone::Head, "Agents may propose versions"),
            t(Tone::Plain, if *b { "An agent's deploy of it is kept as a proposal, for a person to approve." } else { "An agent's deploy of it is refused." }),
            t(Tone::Dim, "enter: turn it the other way"),
        ],
        Setting::AgentsAtOnce(b) => vec![
            t(Tone::Head, "An agent's version deploys at once"),
            t(Tone::Plain, if *b { "An agent's version that asks for nothing more deploys without a person; what asks for more still waits for one." } else { "Each version an agent deploys waits for a person." }),
            t(Tone::Dim, "enter: turn it the other way (on takes flow.approve-deploy, and a host that lets it)"),
        ],
        Setting::Waiting(n) => vec![t(Tone::Head, &format!("{n} waiting")), t(Tone::Dim, "enter: Approvals › Runs waiting, where they are decided")],
        Setting::Session { number, .. } => {
            let s = d.sessions.iter().find(|s| s["number"].as_u64() == Some(*number)).cloned().unwrap_or_default();
            let mut rows = vec![t(Tone::Head, &format!("Session {number}"))];
            rows.push(field("From", s["from"].as_str().unwrap_or("?").to_string()));
            rows.push(field("Started", crate::cmdline::when(s["started_at"].as_u64())));
            rows.push(field("Ended", s["ended_at"].as_u64().map_or("running now".into(), |t| crate::cmdline::when(Some(t)))));
            rows.push(field("Requests", format!("{}, {} failed", s["requests"], s["errors"])));
            for (k, v) in s["state"].as_object().into_iter().flatten() {
                rows.push(field(&format!("  {k}"), holon_author::agent::show(v)));
            }
            if d.entry["status"] == "stopped" && s["ended_at"].is_u64() {
                rows.push(t(Tone::Dim, "enter: run it again from this session's state"));
            }
            rows
        }
        Setting::Remove { forget } => {
            let mut rows = vec![t(Tone::Head, if *forget { "Remove it, and forget its keys" } else { "Remove it" })];
            rows.push(t(Tone::Plain, "Its routes and actions are gone at once. Its runs, history and state stay on record; deploying it again brings it back."));
            if *forget {
                rows.push(t(Tone::Warn, "Its keys are forgotten too: its kept run records, its sealed state and its secrets can't be read again, by anyone (host.operate)."));
            }
            // As the web console removes it: a stopped flow only.
            rows.push(t(Tone::Dim, if d.entry["status"] == "stopped" { "enter: asks for its name, typed" } else { "It is removed once it is stopped: x stops it." }));
            rows
        }
    }
}

impl App {
    /// Enter on a Settings row of `slug`'s page.
    pub(crate) fn setting_enter(&mut self, slug: &str, s: Setting) {
        let status = self.detail.as_ref().map(|d| d.entry["status"].as_str().unwrap_or_default().to_string()).unwrap_or_default();
        let ask = |title: String, lines: Vec<String>, then: Then| Some(Modal::Confirm { title, lines, then });
        let slug_ = slug.to_string();
        self.modal = match s {
            Setting::Head(_) => None,
            Setting::Limit { name, label, unit, now, .. } => Some(Modal::input(
                format!("Lower {label} of {slug}"),
                vec![format!("It holds {} now. A lower one applies at once, from the next run; a higher one is a new version, which a person approves.", said(now, &unit))],
                String::new(),
                false,
                Ask::Lower { slug: slug_, name, now },
            )),
            Setting::PlainHttp { host, held: true } => ask(
                format!("Revoke plain HTTP to {host}?"),
                vec![format!("\"{slug}\" may no longer call {host} unencrypted: a run that does fails. Granting it back is in Access.")],
                Then::Grant { slug: slug_, item: format!("PlainHttp({host:?})"), back: false },
            ),
            Setting::PrivateAddresses { held: true } => ask(
                "Revoke private addresses?".into(),
                vec![format!("\"{slug}\" may no longer reach this host's own network. Granting it back is in Access.")],
                Then::Grant { slug: slug_, item: "PrivateAddresses".into(), back: false },
            ),
            Setting::PlainHttp { .. } | Setting::PrivateAddresses { .. } => return self.fail("Revoked: granting it back is in Access."),
            Setting::KeepRunning(on) => ask(
                format!("{} keep running?", if on { "Turn off" } else { "Turn on" }),
                vec![if on { "When something it requires is revoked, it pauses, and waits for a person to decide.".into() } else { "When something it requires is revoked, it keeps running, and a run that needs it fails.".into() }],
                Then::Settings { slug: slug_, grant: true, body: json!({ "keep_running": !on }), said: format!("keep running is {}", if on { "off" } else { "on" }) },
            ),
            Setting::AgentsPropose(on) => ask(
                format!("{} agents proposing versions of {slug}?", if on { "Stop" } else { "Allow" }),
                vec![if on { "An agent's deploy of it is refused from now on.".into() } else { "An agent's deploy of it is kept as a proposal, for a person to approve.".into() }],
                Then::Settings { slug: slug_, grant: false, body: json!({ "agents_propose": !on }), said: format!("agents {} propose versions", if on { "may no longer" } else { "may" }) },
            ),
            Setting::AgentsAtOnce(on) => ask(
                format!("{} agents' versions of {slug} deploy at once?", if on { "No longer let" } else { "Let" }),
                if on {
                    vec!["Each version an agent deploys waits for a person again.".into()]
                } else {
                    vec![
                        "An agent's version that asks for nothing more would deploy without a person reading it: what it runs changes, with no one's approval.".into(),
                        "What asks for more still waits for one. Turn it on only if you mean to grant this.".into(),
                    ]
                },
                Then::Settings { slug: slug_, grant: false, body: json!({ "agents_deploy_at_once": !on }), said: format!("agents' versions {}", if on { "wait for a person again" } else { "that ask for nothing more deploy at once" }) },
            ),
            Setting::Waiting(_) => return self.show_waiting_run(None),
            Setting::Session { number, ended: Some(_), state: true, .. } if status == "stopped" => ask(
                format!("Run {slug} from session {number}?"),
                vec![format!("A new session starts with the state session {number} ended with. The state it has now stays on record.")],
                Then::Lifecycle(slug_, "run", Some(json!(number))),
            ),
            Setting::Session { .. } if status != "stopped" => return self.fail("A flow runs again from a session once it is stopped (x stops it)."),
            Setting::Session { .. } => return self.fail("That session kept no state to run from."),
            Setting::Remove { .. } if status != "stopped" => return self.fail("A flow is removed once it is stopped (x stops it)."),
            Setting::Remove { forget } => {
                let mut lines = vec!["Its routes and actions are gone at once. Its runs, history and state stay on record.".to_string()];
                if forget {
                    lines.push("Its keys are forgotten too: its kept records, sealed state and secrets can't be read again, by anyone.".into());
                }
                lines.push(format!("Type its name, {slug}, to remove it."));
                Some(Modal::input(format!("Remove {slug}{}?", if forget { ", forgetting its keys" } else { "" }), lines, String::new(), false, Ask::Remove { slug: slug_, forget }))
            }
        };
    }

    /// A limit typed: lowered at once; a higher one, or the same, said so.
    pub(crate) fn lower_limit(&mut self, slug: String, name: String, now: u64, typed: String) {
        let Ok(n) = typed.trim().parse::<u64>() else { return self.fail("a whole number, 0 or more") };
        if n == now {
            return self.tell("the same: nothing changed");
        }
        if n > now {
            return self.fail(format!("{n} is higher than {now}: raising it takes a new version, which a person approves (e opens it in the editor)"));
        }
        self.change_settings(slug, false, json!({ "lower": { name.clone(): n } }), format!("{name} lowered to {n}: it holds from the next run"));
    }

    /// Send a settings change (`…/settings`, or `…/grant` for keep running), then read it again.
    pub(crate) fn change_settings(&mut self, slug: String, grant: bool, body: Value, said: String) {
        let Some(c) = self.client.as_ref() else { return };
        let path = format!("/deployments/{}/{}", seg(&slug), if grant { "grant" } else { "settings" });
        match c.post(&path, body) {
            Ok(_) => {
                self.refresh();
                self.say(said);
            }
            Err(e) => self.fail(e),
        }
    }

    /// Remove `slug`, its name typed; then its keys forgotten, when asked (`POST /admin/keys/forget`,
    /// once it is no longer deployed). A forget the server answers with an error is said as one.
    pub(crate) fn remove_typed(&mut self, slug: String, forget: bool, typed: String) {
        if typed.trim() != slug {
            return self.fail(format!("not removed: type {slug} to remove it"));
        }
        if forget && let Some(Err(why)) = self.client.as_ref().map(|c| c.may("host.operate", None)) {
            return self.fail(format!("not removed: forgetting its keys takes host.operate ({why})"));
        }
        self.remove(&slug);
        if !forget || self.note.as_ref().is_some_and(|n| n.error) {
            return;
        }
        let Some(c) = self.client.as_ref() else { return };
        match c.post("/admin/keys/forget", json!({ "slug": slug })) {
            Ok(v) if v["error"].is_string() => self.fail(format!("removed \"{slug}\", but its keys aren't forgotten: {}", v["error"].as_str().unwrap_or_default())),
            Ok(v) if v["forgotten"] == false => self.say(format!("removed \"{slug}\": {}", v["root"]["why"].as_str().unwrap_or("its keys had nothing to forget"))),
            Ok(_) => self.say(format!("removed \"{slug}\", and forgot its keys: what was sealed under them can't be read again")),
            Err(e) => self.fail(format!("removed \"{slug}\", but its keys aren't forgotten: {e}")),
        }
    }

    /// A secret's version to roll back to, typed: its dry run first, said, then asked.
    pub(crate) fn rollback_typed(&mut self, slug: String, name: String, typed: String) {
        let Ok(version) = typed.trim().trim_start_matches('v').parse::<u64>() else { return self.fail("a version number, as its history lists it") };
        let Some(c) = self.client.as_ref() else { return };
        let path = format!("/deployments/{}/secrets/{}/rollback", seg(&slug), seg(&name));
        match c.post(&path, json!({ "version": version, "dry_run": true })) {
            Ok(v) => {
                let preview = crate::secretcmd::rollback_preview(&name, &v);
                let said = preview.split(". Nothing changed").next().unwrap_or(&preview).to_string();
                self.modal = Some(Modal::Confirm { title: format!("Roll {name} back to v{version}?"), lines: vec![format!("{said}."), "The versions in between stay on record.".into()], then: Then::Rollback { slug, name, version } });
            }
            Err(e) => self.fail(e),
        }
    }

    /// Roll a secret back, as its dry run said.
    pub(crate) fn rollback(&mut self, slug: String, name: String, version: u64) {
        let Some(c) = self.client.as_ref() else { return };
        match c.post(&format!("/deployments/{}/secrets/{}/rollback", seg(&slug), seg(&name)), json!({ "version": version })) {
            Ok(v) => {
                self.refresh();
                self.say(format!("{name} is v{} again, as v{}", version, v["version"]));
            }
            Err(e) => self.fail(e),
        }
    }
}
