//! What `holon editor` reads from and sends to a server: the same requests the browser's editor
//! makes, as the person signed in (`deploycmd::Person`), so a deploy from here approves.

use holon_author::agent;
use serde_json::{json, Value};

use crate::deploycmd::{self, Staged};
use crate::mcp::{seg, Api, Body};

/// A server, and what it answered last.
pub(crate) struct Client {
    pub(crate) api: Box<dyn Api>,
    /// Who it signs us in as, if it says.
    pub(crate) me: Option<String>,
    /// And with what role (`admin`, `author`, …), if it says.
    pub(crate) role: Option<String>,
    pub(crate) deployments: Vec<Value>,
    /// Saved flows (`GET /flows`): name, saved_by, saved_at_epoch_seconds.
    pub(crate) flows: Vec<Value>,
    /// What was done on the server lately, newest first (`GET /activity`): at, action, target, by.
    pub(crate) activity: Vec<Value>,
    /// Each deployment's latest runs, oldest first, with the run count they were read at: read
    /// again only when it has run since, for the lists' sparks and last runs.
    pub(crate) recent: std::collections::HashMap<String, (u64, Vec<Value>)>,
    /// Each deployment's run log, as last read with its runs: how many lines don't follow the
    /// one before (changed outside holon), and its head.
    pub(crate) chains: std::collections::HashMap<String, (usize, String)>,
    /// What the host warns of (`GET /admin/warnings`, storage.rs): says, todo. An admin's only.
    pub(crate) warnings: Vec<Value>,
    /// The access document's version as `/auth/me` said it last: what `may` answers changes only
    /// with it (or a flow's tags, or a minute on), so its answers are read again when it moves.
    pub(crate) access_version: Option<u64>,
    /// The runs waiting at a point that the caller may see (`GET /runs/waiting`), oldest first,
    /// each with its `slug`; and why they couldn't be read, when they couldn't.
    pub(crate) waiting_runs: Vec<Value>,
    pub(crate) waiting_runs_error: Option<String>,
    /// The versions proposed and waiting for a person (`GET /proposals`), oldest first: each
    /// replaces its flow's "a version saved since" wait, which it saved.
    pub(crate) proposals: Vec<Value>,
    /// The access changes, newest first (`GET /access/proposals`), for whoever may see people; or
    /// why they aren't read.
    pub(crate) access_changes: Vec<Value>,
    pub(crate) access_changes_error: Option<String>,
    /// What the caller may do (`GET /auth/me/can`), by flow ("" for the host and every flow at
    /// once), with when it was asked: each action's answer, or the words of its refusal.
    pub(crate) can: std::collections::HashMap<String, (std::time::Instant, Answers)>,
    /// The host at a glance (`GET /summary?hours=24`), of the flows the caller may see the runs
    /// of: each one's hours, median and newest run, the newest runs, and the run logs' chain and
    /// witness. Null when it can't be read; a flow it doesn't cover is read run by run.
    pub(crate) summary: Value,
}

/// Each action's answer, or the words of its refusal.
pub(crate) type Answers = std::collections::HashMap<String, Result<(), String>>;

/// How long what the caller may do is kept before it is asked again, as the web console keeps it.
const CAN_KEPT: std::time::Duration = std::time::Duration::from_secs(60);

/// One deployment's pages: what it may do, its runs, its history, its access.
pub(crate) struct Detail {
    pub(crate) slug: String,
    pub(crate) entry: Value,
    pub(crate) manifest: Value,
    /// Its newest runs, oldest first.
    pub(crate) runs: Vec<Value>,
    /// What was done to it, oldest first (`…/events`).
    pub(crate) events: Vec<Value>,
    /// Its versions, newest first (`…/versions`).
    pub(crate) versions: Vec<Value>,
    /// Its grant as `…/grant` answers it (`{grant, hash, attention}`), as `holon grant` reads it.
    pub(crate) grant: Value,
    /// Its secrets as `…/secrets` answers it, with the ones it reads (`reads`), as `holon secret
    /// list` reads it.
    pub(crate) secrets: Value,
    /// Runs parked, waiting for something (`…/runs`).
    pub(crate) parked: Vec<Value>,
    /// What it holds in memory and on disk (`…/holds`, storage.rs); null from an older server.
    pub(crate) holds: Value,
    /// Its run log's head (the hash of its newest line), and the seqs of the runs that don't
    /// follow the one before them: what a run's record says of its chain.
    pub(crate) head: String,
    pub(crate) breaks: Vec<u64>,
    /// The host's policy (`GET /permissions`): its limits' defaults and ceilings, as Settings says
    /// them; null for whoever may not read it (policy.view).
    pub(crate) policy: Value,
    /// Its sessions, oldest first (`…/sessions`), each with the state it ended with.
    pub(crate) sessions: Vec<Value>,
    /// What it is connected to (state.view's): the feeds its handlers listen to (`…/feeds`), the
    /// queues it listens to and sends to (`…/queues`: `{listening, sending}`), and its channels
    /// (`…/channels`). Empty for whoever may not read them.
    pub(crate) feeds: Vec<Value>,
    pub(crate) queues: Value,
    pub(crate) channels: Vec<Value>,
}

/// One moment of a deployment's history: a run, or something done to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Moment {
    /// Its index in `Detail::runs`.
    Run(usize),
    /// Its index in `Detail::events`.
    Event(usize),
}

/// A point to go back to, and what can be taken from it.
#[derive(Clone, Debug)]
pub(crate) struct Point {
    /// As the restore's body names it: `{"run": seq}` or `{"version": n}`.
    pub(crate) at: Value,
    pub(crate) name: String,
    /// Why its code, or its state, can't be put back, when it can't.
    pub(crate) code: Result<(), String>,
    pub(crate) state: Result<(), String>,
}

/// How many runs a flow's pages show.
pub(crate) const RUNS: u64 = 200;
/// How many of its latest runs each deployment's row reads, for its spark and last run.
pub(crate) const RECENT: u64 = 50;
/// A page of a collection: its items, (key, value), and the key the next starts after.
pub(crate) type Items = (Vec<(String, Value)>, Option<String>);

/// How many of a collection's items a page of the State section shows.
pub(crate) const PAGE: usize = 100;

fn list(v: &Value, key: &str) -> Vec<Value> {
    v.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
}

pub(crate) fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|i| i.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

impl Client {
    pub(crate) fn new(api: Box<dyn Api>) -> Client {
        Client { api, me: None, role: None, deployments: vec![], flows: vec![], activity: vec![], recent: Default::default(), chains: Default::default(), warnings: vec![], access_version: None, can: Default::default(), waiting_runs: vec![], waiting_runs_error: None, proposals: vec![], access_changes: vec![], access_changes_error: None, summary: Value::Null }
    }

    pub(crate) fn server(&self) -> String {
        self.api.server()
    }

    pub(crate) fn get(&self, path: &str) -> Result<Value, String> {
        deploycmd::call(self.api.as_ref(), "GET", path, None)
    }

    pub(crate) fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        deploycmd::call(self.api.as_ref(), "POST", path, Some(Body::Json(body)))
    }

    pub(crate) fn patch(&self, path: &str, body: Value) -> Result<Value, String> {
        deploycmd::call(self.api.as_ref(), "PATCH", path, Some(Body::Json(body)))
    }

    pub(crate) fn put(&self, path: &str, body: Value) -> Result<Value, String> {
        deploycmd::call(self.api.as_ref(), "PUT", path, Some(Body::Json(body)))
    }

    pub(crate) fn delete(&self, path: &str) -> Result<Value, String> {
        deploycmd::call(self.api.as_ref(), "DELETE", path, None)
    }

    /// Read the deployments, saved flows and activity again.
    pub(crate) fn refresh(&mut self) -> Result<(), String> {
        // Who we are, and the access document's version: when it moves, what we may do is asked
        // again (a server without accounts answers no version).
        let me = self.get("/auth/me").ok();
        self.me = me.as_ref().and_then(|m| m.get("name").and_then(Value::as_str).map(str::to_string));
        self.role = me.as_ref().and_then(|m| m.get("role").and_then(Value::as_str).map(str::to_string));
        let version = me.as_ref().and_then(|m| m["access_version"].as_u64());
        if version != self.access_version {
            self.can.clear();
            self.access_version = version;
        }
        self.ask_can(None);
        // The flows, unless the server said the caller may see none (an admin of people alone, say):
        // then none are listed, and the rest is read as ever.
        let (deployments, flows) = if self.refused("flow.view").is_some() {
            (vec![], vec![])
        } else {
            (list(&self.get("/deployments")?, "deployments"), self.get("/flows")?.as_array().cloned().unwrap_or_default())
        };
        // Waiting ones come first in the answer, oldest first; whoever may not list flows reads none.
        self.proposals = self.get("/proposals").map(|v| list(&v, "proposals")).unwrap_or_default().into_iter().filter(|p| p["state"] == "waiting").collect();
        match self.get("/runs/waiting") {
            Ok(v) => {
                let mut runs = list(&v, "runs");
                runs.sort_by_key(|r| r["parked_at"].as_u64().unwrap_or(0));
                (self.waiting_runs, self.waiting_runs_error) = (runs, None);
            }
            Err(e) => (self.waiting_runs, self.waiting_runs_error) = (vec![], Some(e)),
        }
        (self.access_changes, self.access_changes_error) = match self.may("people.view", None) {
            Err(why) => (vec![], Some(why)),
            Ok(()) => match self.get("/access/proposals") {
                Ok(v) => (list(&v, "proposals"), None),
                Err(e) => (vec![], Some(e)),
            },
        };
        // Only an admin may read it all; for anyone else it stays empty.
        self.activity = self.get("/activity?limit=30").map(|a| list(&a, "entries")).unwrap_or_default();
        self.warnings = self.get("/admin/warnings").map(|w| list(&w, "warnings")).unwrap_or_default();
        // The host at a glance, in one read: each flow it covers needs no read of its own.
        self.summary = self.get("/summary?hours=24").unwrap_or(Value::Null);
        let covered: Vec<(String, usize, String)> = self.summary["deployments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| Some((f["slug"].as_str()?.to_string(), f["breaks"].as_array().map_or(0, Vec::len), f["head"].as_str().unwrap_or_default().to_string())))
            .collect();
        // The latest runs of what ran since it was last read; what is gone is forgotten.
        self.recent.retain(|slug, _| deployments.iter().any(|d| d["slug"].as_str() == Some(slug.as_str())) && !covered.iter().any(|(s, ..)| s == slug));
        self.chains.retain(|slug, _| deployments.iter().any(|d| d["slug"].as_str() == Some(slug.as_str())));
        for (slug, breaks, head) in &covered {
            self.chains.insert(slug.clone(), (*breaks, head.clone()));
        }
        for d in &deployments {
            let (Some(slug), runs) = (d["slug"].as_str(), d["trigger_count"].as_u64().unwrap_or(0)) else { continue };
            if covered.iter().any(|(s, ..)| s == slug) {
                continue;
            }
            if self.recent.get(slug).is_some_and(|(n, _)| *n == runs) {
                continue;
            }
            let log = if runs == 0 { None } else { self.get(&format!("/deployments/{}/log?limit={RECENT}", seg(slug))).ok() };
            let read = log.as_ref().map(|l| list(l, "entries")).unwrap_or_default();
            if let Some(l) = &log {
                self.chains.insert(slug.to_string(), (l["breaks"].as_array().map_or(0, Vec::len), l["head"].as_str().unwrap_or_default().to_string()));
            }
            self.recent.insert(slug.to_string(), (runs, read));
        }
        self.deployments = deployments;
        self.flows = flows;
        Ok(())
    }

    /// Ask what the caller may do, every action at once, on `flow` or on the host and every flow,
    /// unless it was asked within the minute. Answers are kept by this client, which signs in with
    /// one token for its life (another server, or another sign-in, is another client), so an
    /// answer is never another caller's. One that can't be read is kept as no answer.
    pub(crate) fn ask_can(&mut self, flow: Option<&str>) {
        let key = flow.unwrap_or_default().to_string();
        if self.can.get(&key).is_some_and(|(at, _)| at.elapsed() < CAN_KEPT) {
            return;
        }
        let actions: Vec<&str> = crate::can::Action::ALL.iter().map(|a| a.name()).collect();
        let mut path = format!("/auth/me/can?actions={}", actions.join(","));
        if let Some(f) = flow {
            path.push_str(&format!("&flow={}", seg(f)));
        }
        let answers = match self.get(&path) {
            Ok(Value::Object(m)) => m.into_iter().map(|(k, v)| (k, if v == Value::Bool(true) { Ok(()) } else { Err(v.as_str().unwrap_or("not yours to do here").to_string()) })).collect(),
            Ok(_) => Default::default(),
            Err(e) => [(String::new(), Err(e))].into(),
        };
        self.can.insert(key, (std::time::Instant::now(), answers));
    }

    /// The server's no to `action` on the host and every flow, when it answered one: not when it
    /// hasn't answered, which [`Client::may`] takes as a no too.
    pub(crate) fn refused(&self, action: &str) -> Option<String> {
        self.can.get("").and_then(|(_, a)| a.get(action)).and_then(|a| a.clone().err())
    }

    /// Whether the caller may do `action` (a catalogue name, `people.manage`), on `flow` or on the
    /// host and every flow: the refusal's words when not. Not asked yet, or not answered, is no
    /// (R77): a key never works on a guess, only on the server's yes.
    pub(crate) fn may(&self, action: &str, flow: Option<&str>) -> Result<(), String> {
        let asked = flow.and_then(|f| self.can.get(f)).or_else(|| self.can.get(""));
        match asked.map(|(_, a)| (a.get(action), a.get(""))) {
            Some((Some(answer), _)) => answer.clone(),
            Some((None, Some(Err(e)))) => Err(format!("the server didn't say what you may do ({e}): r asks again")),
            _ => Err("the server hasn't said yet what you may do: r asks again".into()),
        }
    }

    pub(crate) fn deployment(&self, slug: &str) -> Option<&Value> {
        self.deployments.iter().find(|d| d["slug"].as_str() == Some(slug))
    }

    pub(crate) fn saved(&self, name: &str) -> Option<&Value> {
        self.flows.iter().find(|f| f["name"].as_str() == Some(name))
    }

    /// The version of `slug` proposed and waiting for a person, if one is.
    pub(crate) fn proposal_for(&self, slug: &str) -> Option<&Value> {
        self.proposals.iter().find(|p| p["slug"].as_str() == Some(slug))
    }

    /// What waits for a person, deployment by deployment, then the saved flows never deployed,
    /// then what is proposed of a flow neither. A version proposed is said as one, in place of
    /// the save it made.
    pub(crate) fn waiting(&self) -> Vec<(String, String)> {
        let mut out = vec![];
        for d in &self.deployments {
            let slug = d["slug"].as_str().unwrap_or("?").to_string();
            let proposed = self.proposal_for(&slug);
            for w in crate::mcp::waits_for(d, self.saved(&slug)) {
                if proposed.is_none() || !w.starts_with("to review a version saved since") {
                    out.push((slug.clone(), w));
                }
            }
            if let Some(p) = proposed {
                out.push((slug.clone(), super::approvals::proposal_wait(p)));
            }
        }
        for f in &self.flows {
            let name = f["name"].as_str().unwrap_or("?");
            if let Some(p) = self.proposal_for(name).filter(|_| self.deployment(name).is_none()) {
                out.push((name.to_string(), super::approvals::proposal_wait(p)));
            } else if self.deployment(name).is_none() {
                let by = f["saved_by"].as_str().unwrap_or("someone");
                out.push((name.to_string(), format!("to deploy it for the first time: saved by {by}, at {}", crate::mcp::when(&f["saved_at_epoch_seconds"]))));
            }
        }
        // A proposal of a flow neither deployed nor saved here waits all the same.
        for p in &self.proposals {
            let name = p["slug"].as_str().unwrap_or("?");
            if self.deployment(name).is_none() && !self.flows.iter().any(|f| f["name"].as_str() == Some(name)) {
                out.push((name.to_string(), super::approvals::proposal_wait(p)));
            }
        }
        out
    }

    pub(crate) fn detail(&self, slug: &str) -> Result<Detail, String> {
        let path = format!("/deployments/{}", seg(slug));
        let entry = self.deployment(slug).cloned().ok_or_else(|| format!("nothing named \"{slug}\" is deployed on {}", self.server()))?;
        let manifest = self.get(&format!("{path}/manifest"))?;
        // A run's steps and values are run.steps' (R55). To whoever holds run.view alone, the list
        // of runs as /log/heads says it, so the page opens and shows what they hold: the runs,
        // the log lines, the history; a run's own tabs then say what it can't show.
        let log = match self.get(&format!("{path}/log?limit={RUNS}")) {
            Err(e) if e.contains("run.steps") => self.get(&format!("{path}/log/heads?limit={RUNS}"))?,
            log => log?,
        };
        let runs = list(&log, "entries");
        let head = log["head"].as_str().unwrap_or_default().to_string();
        let breaks = log["breaks"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
        let events = list(&self.get(&format!("{path}/events"))?, "events");
        let versions = list(&self.get(&format!("{path}/versions"))?, "versions");
        // What an older server, or a viewer, can't read stays empty.
        let grant = self.get(&format!("{path}/grant")).unwrap_or(Value::Null);
        let mut secrets = self.get(&format!("{path}/secrets")).unwrap_or_else(|_| json!({}));
        secrets["reads"] = json!(crate::secretcmd::reads(&manifest));
        let parked = if entry["waiting"].as_u64().unwrap_or_default() > 0 { self.get(&format!("{path}/runs")).map(|r| list(&r, "runs")).unwrap_or_default() } else { vec![] };
        let holds = self.get(&format!("{path}/holds")).unwrap_or(Value::Null);
        let policy = if self.may("policy.view", None).is_ok() { self.get("/permissions").unwrap_or(Value::Null) } else { Value::Null };
        let sessions = self.get(&format!("{path}/sessions")).map(|s| list(&s, "sessions")).unwrap_or_default();
        let feeds = self.get(&format!("{path}/feeds")).map(|f| list(&f, "feeds")).unwrap_or_default();
        let queues = self.get(&format!("{path}/queues")).unwrap_or(Value::Null);
        let channels = self.get(&format!("{path}/channels")).map(|c| list(&c, "channels")).unwrap_or_default();
        Ok(Detail { slug: slug.to_string(), entry, manifest, runs, events, versions, grant, secrets, parked, holds, head, breaks, policy, sessions, feeds, queues, channels })
    }

    /// Its text as deployed now, or as version `unit` was.
    pub(crate) fn text(&self, slug: &str, unit: Option<&str>) -> Result<String, String> {
        let path = match unit.filter(|u| !u.is_empty()) {
            Some(u) => format!("/deployments/{}/document?unit={u}", seg(slug)),
            None => format!("/deployments/{}/document", seg(slug)),
        };
        agent::text_of(slug, &self.get(&path)?)
    }

    /// The text of a saved flow: as written when that is its canvas's code, else its canvas
    /// printed (`agent::text_of`, as every reading of a document's text is); and the stamp of the
    /// save it is.
    pub(crate) fn saved_text(&self, name: &str) -> Result<(String, Option<u64>), String> {
        let saved = self.get(&format!("/flows/{}", seg(name)))?;
        let at = saved["saved_at_epoch_seconds"].as_u64();
        let document = saved.get("document").cloned().unwrap_or(saved);
        agent::text_of(name, &document).map(|text| (text, at))
    }

    /// What a run did, laid over the text of the version it ran on.
    pub(crate) fn run_view(&self, slug: &str, entry: &Value) -> Result<agent::RunView, String> {
        let path = match entry.get("unit").and_then(Value::as_str).filter(|u| !u.is_empty()) {
            Some(u) => format!("/deployments/{}/document?unit={u}", seg(slug)),
            None => format!("/deployments/{}/document", seg(slug)),
        };
        agent::run_view(slug, &self.get(&path)?, entry)
    }

    /// Start, pause or stop a deployment; `from` starts a stopped one from a session's state.
    pub(crate) fn lifecycle(&self, slug: &str, action: &str, from: Option<Value>) -> Result<(), String> {
        let body = from.map(|f| json!({ "from": f })).unwrap_or_else(|| json!({}));
        self.post(&format!("/deployments/{}/{action}", seg(slug)), body).map(|_| ())
    }

    /// A saved flow deleted (`DELETE /flows/{name}`, flow.delete): its deployment, if any, stays.
    pub(crate) fn delete_saved(&self, name: &str) -> Result<(), String> {
        self.delete(&format!("/flows/{}", seg(name))).map(|_| ())
    }

    pub(crate) fn remove(&self, slug: &str) -> Result<(), String> {
        self.delete(&format!("/deployments/{}", seg(slug))).map(|_| ())
    }

    pub(crate) fn stage(&self, plan: &agent::Plan, slug: &str) -> Result<Staged, String> {
        deploycmd::stage(self.api.as_ref(), plan, slug)
    }

    /// Deploy what was staged, approving what it needs as us, with its text as written; the
    /// proposal it was kept as instead, when what it needs is beyond us.
    pub(crate) fn commit(&self, plan: &agent::Plan, staged: Staged, text: &str) -> Result<Option<Value>, String> {
        deploycmd::commit(self.api.as_ref(), plan, staged, crate::mcp::saved_document(plan, text))
    }

    /// Save a flow without deploying it, as the flows page lists it, over `base` when said (the save
    /// it replaces, flows.rs `save`): the save's stamp.
    pub(crate) fn save(&self, name: &str, plan: &agent::Plan, text: &str, base: Option<Option<u64>>) -> Result<u64, String> {
        let mut body = json!({ "name": name, "document": crate::mcp::saved_document(plan, text) });
        if let Some(base) = base {
            body["base"] = json!(base);
        }
        let saved = self.post("/flows", body)?;
        saved["saved_at_epoch_seconds"].as_u64().ok_or_else(|| "the server didn't say when it saved it".to_string())
    }

    /// A restore: a dry run first, then (`acknowledge` set) the real one.
    pub(crate) fn restore(&self, slug: &str, at: &Value, code: bool, state: bool, acknowledge: Option<&[String]>) -> Result<Value, String> {
        let mut body = json!({ "code": code, "state": state });
        for (k, v) in at.as_object().into_iter().flatten() {
            body[k] = v.clone();
        }
        match acknowledge {
            Some(a) => body["acknowledge"] = json!(a),
            None => body["dry_run"] = json!(true),
        }
        self.post(&format!("/deployments/{}/restore", seg(slug)), body)
    }

    /// Version `n`'s text beside the text deployed now, as `holon restore --dry-run` prints it.
    pub(crate) fn code_diff(&self, slug: &str, n: u64) -> Result<String, String> {
        crate::mcp::code_diff(self.api.as_ref(), slug, n)
    }

    /// The text deployed now and version `n`'s, printed alike; a note when without their names.
    pub(crate) fn code_texts(&self, slug: &str, n: u64) -> Result<(String, String, &'static str), String> {
        crate::mcp::code_texts(self.api.as_ref(), slug, n)
    }

    /// Change a grant: revoke items, or grant revoked ones back.
    pub(crate) fn change_grant(&self, slug: &str, revoke: &[String], grant: &[String]) -> Result<(), String> {
        self.post(&format!("/deployments/{}/grant", seg(slug)), json!({ "revoke": revoke, "grant": grant })).map(|_| ())
    }

    /// Set a secret to a value typed here (never shown again, here or anywhere).
    pub(crate) fn set_secret(&self, slug: &str, name: &str, value: &str) -> Result<Value, String> {
        self.post(&format!("/deployments/{}/secrets/{}", seg(slug), seg(name)), json!({ "value": value }))
    }

    /// A secret read from elsewhere: `env:HOLON_SECRET_…` or `vault:<mount>/<path>#<field>`.
    pub(crate) fn set_secret_from(&self, slug: &str, name: &str, from: &str) -> Result<Value, String> {
        self.post(&format!("/deployments/{}/secrets/{}", seg(slug), seg(name)), json!({ "from": from }))
    }

    /// The `HOLON_SECRET_…` names the server reads (its environment and its `--dotenv` file),
    /// never their values, and that file; nothing for one who isn't an admin.
    pub(crate) fn env_names(&self) -> (Vec<String>, Option<String>) {
        match self.get("/admin/secrets/env") {
            Ok(v) => (v["names"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect(), v["dotenv"].as_str().map(str::to_string)),
            Err(_) => (vec![], None),
        }
    }

    pub(crate) fn remove_secret(&self, slug: &str, name: &str) -> Result<(), String> {
        self.delete(&format!("/deployments/{}/secrets/{}", seg(slug), seg(name))).map(|_| ())
    }

    /// Its state's cells, as the server keeps them (`…/state`): key to value.
    pub(crate) fn state(&self, slug: &str) -> Result<Value, String> {
        self.get(&format!("/deployments/{}/state", seg(slug))).map(|s| s["state"].clone())
    }

    /// Its log lines, oldest first (`…/logs`): at a level or louder, and with a run, up to the end of
    /// it, or only its own (`(seq, own)`). Refused to an agent.
    pub(crate) fn logs(&self, slug: &str, level: Option<&str>, run: Option<(u64, bool)>) -> Result<Vec<Value>, String> {
        let mut asked = String::from("limit=500");
        if let Some(level) = level {
            asked.push_str(&format!("&level={level}"));
        }
        if let Some((seq, own)) = run {
            asked.push_str(&format!("&{}={seq}", if own { "run" } else { "until" }));
        }
        self.get(&format!("/deployments/{}/logs?{asked}", seg(slug))).map(|a| a["lines"].as_array().cloned().unwrap_or_default())
    }

    /// A page of a collection's items, in key order, after the key `after` (from its first
    /// without one): (key, value), and the key the next page starts after, when there may be more.
    pub(crate) fn collection(&self, slug: &str, name: &str, after: Option<&str>) -> Result<Items, String> {
        let after = after.map(|a| format!("&after={}", seg(a))).unwrap_or_default();
        let c = self.get(&format!("/deployments/{}/collections/{}?limit={PAGE}{after}", seg(slug), seg(name)))?;
        let items = c["items"].as_array().into_iter().flatten().filter_map(|i| Some((i.get(0)?.as_str()?.to_string(), i.get(1).cloned().unwrap_or(Value::Null)))).collect();
        Ok((items, c["next"].as_str().map(str::to_string)))
    }

    /// The runs that changed a cell (or a collection, or one item of it), as `holon history --cell`.
    pub(crate) fn changes(&self, slug: &str, name: &str, item: Option<&str>) -> Result<Vec<String>, String> {
        let item = item.map(|i| format!("&item={}", seg(i))).unwrap_or_default();
        let answer = self.get(&format!("/deployments/{}/changes?name={}{item}&limit=50", seg(slug), seg(name)))?;
        let versions = crate::history::Versions::of(&self.get(&format!("/deployments/{}/versions", seg(slug)))?);
        Ok(crate::history::change_lines(&answer, &versions))
    }

    /// Send a deployment one HTTP request, as `holon call` does: `{status, headers, body}`.
    pub(crate) fn request(&self, slug: &str, method: &str, url: &str, body: &str) -> Result<Value, String> {
        let headers: Vec<(&str, &str)> = if body.trim_start().starts_with(['{', '[']) { vec![("content-type", "application/json")] } else { vec![] };
        self.post(&format!("/deployments/{}/request", seg(slug)), json!({ "method": method, "url": url, "headers": headers, "body": body }))
    }

    /// Run one of its actions with `input` (JSON), as `holon action` does: its lines, as that
    /// prints them (`true` for what it says on stderr), and whether it went wrong.
    pub(crate) fn action(&self, slug: &str, name: &str, input: &str) -> Result<(Vec<(bool, String)>, bool), String> {
        let input = if input.trim().is_empty() { "{}" } else { input };
        let value: Value = serde_json::from_str(input).map_err(|e| format!("the input is a JSON value ({{\"amount\": 30}}): {e}"))?;
        let (status, body) = self.api.send("POST", &format!("/trigger/{}/{}", seg(slug), seg(name)), Some(Body::Json(value)))?;
        let (lines, code) = crate::opscmd::action_lines(status, &String::from_utf8_lossy(&body));
        Ok((lines, code != 0))
    }

    /// Fire one of its clocks now, as `holon tick` does: what it produced and did.
    pub(crate) fn tick(&self, slug: &str, clock: &str) -> Result<Vec<String>, String> {
        self.post(&format!("/deployments/{}/tick", seg(slug)), json!({ "clock": clock })).map(|v| crate::opscmd::produced_lines(&v))
    }

    /// A secret's versions and what was done to it, as `holon secret history` prints them.
    pub(crate) fn secret_history(&self, slug: &str, name: &str) -> Result<Vec<String>, String> {
        self.get(&format!("/deployments/{}/secrets/{}", seg(slug), seg(name))).map(|v| crate::secretcmd::history_lines(&v))
    }
}

impl Detail {
    /// Its runs as its Runs list shows them, newest first, by their place in `runs`: those with
    /// `filter` in them (their seq, what came in, the route, the answer, the error).
    pub(crate) fn shown_runs(&self, filter: &str) -> Vec<usize> {
        let filter = filter.to_lowercase();
        (0..self.runs.len())
            .rev()
            .filter(|&i| {
                if filter.is_empty() {
                    return true;
                }
                let r = &self.runs[i];
                let said = format!("#{} {} {} {} {} {}", r["seq"], super::flow::came_in(r), r["route"].as_str().unwrap_or_default(), r["status"], r["error"].as_str().unwrap_or_default(), if super::flow::failed(r) { "failed" } else { "ok" });
                said.to_lowercase().contains(&filter)
            })
            .collect()
    }

    /// A run's place on its log's chain: the hash of its line (the next run names it as the one
    /// before; the newest's is the head), the one before it, and whether it follows that one.
    pub(crate) fn on_record(&self, r: &Value) -> (Option<String>, String, bool) {
        let seq = r["seq"].as_u64();
        let at = self.runs.iter().position(|x| x["seq"].as_u64() == seq);
        let hash = match at {
            Some(i) if i + 1 < self.runs.len() => self.runs[i + 1]["prev"].as_str().map(str::to_string),
            Some(_) if !self.head.is_empty() => Some(self.head.clone()),
            _ => None,
        };
        let prev = r["prev"].as_str().unwrap_or_default().to_string();
        (hash, prev, !seq.is_some_and(|s| self.breaks.contains(&s)))
    }

    /// Its history, newest first: each event after (above) the run whose seq it names.
    pub(crate) fn moments(&self) -> Vec<Moment> {
        let mut items: Vec<(u64, u8, Moment)> = self.events.iter().enumerate().map(|(i, e)| (e["run_seq"].as_u64().unwrap_or_default(), 0, Moment::Event(i))).collect();
        items.extend(self.runs.iter().enumerate().map(|(i, r)| (r["seq"].as_u64().unwrap_or_default(), 1, Moment::Run(i))));
        items.sort_by_key(|(seq, kind, _)| (*seq, *kind));
        items.into_iter().rev().map(|(_, _, m)| m).collect()
    }

    pub(crate) fn version(&self, n: u64) -> Option<&Value> {
        self.versions.iter().find(|v| v["number"].as_u64() == Some(n))
    }

    /// The version a run ran on, by its code.
    pub(crate) fn ran_on(&self, run: &Value) -> Option<u64> {
        let versions = crate::history::Versions::of(&json!({ "versions": self.versions }));
        versions.ran_on(run["unit"].as_str()?, run["seq"].as_u64()?)
    }

    /// What going back to a moment can take: a run's state and the code it ran on, or a version's.
    pub(crate) fn point(&self, m: Moment) -> Option<Point> {
        let current = self.entry["unit"].as_str().unwrap_or_default();
        match m {
            Moment::Run(i) => {
                let r = &self.runs[i];
                let seq = r["seq"].as_u64()?;
                let code = match r["unit"].as_str() {
                    None | Some("") => Err("this run doesn't say which code it ran on".to_string()),
                    Some(u) if u == current => Err("it ran on the code deployed now".to_string()),
                    Some(_) => Ok(()),
                };
                let state = if r.get("state_after").is_some_and(|s| !s.is_null()) { Ok(()) } else { Err("this run kept no state to go back to".to_string()) };
                Some(Point { at: json!({ "run": seq }), name: format!("run #{seq}"), code, state })
            }
            Moment::Event(i) => self.version_point(self.events[i]["version"].as_u64()?),
        }
    }

    pub(crate) fn version_point(&self, n: u64) -> Option<Point> {
        let v = self.version(n)?;
        let now = v["current"] == true;
        let code = if now {
            Err("it is the version deployed now".to_string())
        } else if v["restorable"] != true {
            Err("it is from before rc.30: it opens, but doesn't restore".to_string())
        } else {
            Ok(())
        };
        let state = if now {
            Err("it is the version deployed now".to_string())
        } else if v["state_kept"] != true {
            Err("no state was kept with it".to_string())
        } else {
            Ok(())
        };
        Some(Point { at: json!({ "version": n }), name: format!("version {n}"), code, state })
    }
}
