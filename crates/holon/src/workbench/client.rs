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
    pub(crate) deployments: Vec<Value>,
    /// Saved flows (`GET /flows`): name, saved_by, saved_at_epoch_seconds.
    pub(crate) flows: Vec<Value>,
    /// What was done on the server lately, newest first (`GET /activity`): at, action, target, by.
    pub(crate) activity: Vec<Value>,
}

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

fn list(v: &Value, key: &str) -> Vec<Value> {
    v.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
}

pub(crate) fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|i| i.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

impl Client {
    pub(crate) fn new(api: Box<dyn Api>) -> Client {
        Client { api, me: None, deployments: vec![], flows: vec![], activity: vec![] }
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

    pub(crate) fn delete(&self, path: &str) -> Result<Value, String> {
        deploycmd::call(self.api.as_ref(), "DELETE", path, None)
    }

    /// Read the deployments, saved flows and activity again.
    pub(crate) fn refresh(&mut self) -> Result<(), String> {
        let deployments = list(&self.get("/deployments")?, "deployments");
        let flows = self.get("/flows")?.as_array().cloned().unwrap_or_default();
        if self.me.is_none() {
            self.me = self.get("/auth/me").ok().and_then(|m| m.get("name").and_then(Value::as_str).map(str::to_string));
        }
        // Only an admin may read it all; for anyone else it stays empty.
        self.activity = self.get("/activity?limit=30").map(|a| list(&a, "entries")).unwrap_or_default();
        self.deployments = deployments;
        self.flows = flows;
        Ok(())
    }

    pub(crate) fn deployment(&self, slug: &str) -> Option<&Value> {
        self.deployments.iter().find(|d| d["slug"].as_str() == Some(slug))
    }

    pub(crate) fn saved(&self, name: &str) -> Option<&Value> {
        self.flows.iter().find(|f| f["name"].as_str() == Some(name))
    }

    /// What waits for a person, deployment by deployment, then the saved flows never deployed.
    pub(crate) fn waiting(&self) -> Vec<(String, String)> {
        let mut out = vec![];
        for d in &self.deployments {
            let slug = d["slug"].as_str().unwrap_or("?").to_string();
            for w in crate::mcp::waits_for(d, self.saved(&slug)) {
                out.push((slug.clone(), w));
            }
        }
        for f in &self.flows {
            let name = f["name"].as_str().unwrap_or("?");
            if self.deployment(name).is_none() {
                let by = f["saved_by"].as_str().unwrap_or("someone");
                out.push((name.to_string(), format!("to deploy it for the first time: saved by {by}, at {}", crate::mcp::when(&f["saved_at_epoch_seconds"]))));
            }
        }
        out
    }

    pub(crate) fn detail(&self, slug: &str) -> Result<Detail, String> {
        let path = format!("/deployments/{}", seg(slug));
        let entry = self.deployment(slug).cloned().ok_or_else(|| format!("nothing named \"{slug}\" is deployed on {}", self.server()))?;
        let manifest = self.get(&format!("{path}/manifest"))?;
        let runs = list(&self.get(&format!("{path}/log?limit={RUNS}"))?, "entries");
        let events = list(&self.get(&format!("{path}/events"))?, "events");
        let versions = list(&self.get(&format!("{path}/versions"))?, "versions");
        // What an older server, or a viewer, can't read stays empty.
        let grant = self.get(&format!("{path}/grant")).unwrap_or(Value::Null);
        let mut secrets = self.get(&format!("{path}/secrets")).unwrap_or_else(|_| json!({}));
        secrets["reads"] = json!(crate::secretcmd::reads(&manifest));
        let parked = if entry["waiting"].as_u64().unwrap_or_default() > 0 { self.get(&format!("{path}/runs")).map(|r| list(&r, "runs")).unwrap_or_default() } else { vec![] };
        Ok(Detail { slug: slug.to_string(), entry, manifest, runs, events, versions, grant, secrets, parked })
    }

    /// Its text as deployed now, or as version `unit` was.
    pub(crate) fn text(&self, slug: &str, unit: Option<&str>) -> Result<String, String> {
        let path = match unit.filter(|u| !u.is_empty()) {
            Some(u) => format!("/deployments/{}/document?unit={u}", seg(slug)),
            None => format!("/deployments/{}/document", seg(slug)),
        };
        agent::text_of(slug, &self.get(&path)?)
    }

    /// The text a saved flow was saved with: as written when kept (`source`), else printed.
    pub(crate) fn saved_text(&self, name: &str) -> Result<String, String> {
        let saved = self.get(&format!("/flows/{}", seg(name)))?;
        let document = saved.get("document").cloned().unwrap_or(saved);
        match document.get("source").and_then(Value::as_str) {
            Some(s) => Ok(s.to_string()),
            None => agent::text_of(name, &document),
        }
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

    pub(crate) fn remove(&self, slug: &str) -> Result<(), String> {
        self.delete(&format!("/deployments/{}", seg(slug))).map(|_| ())
    }

    pub(crate) fn stage(&self, plan: &agent::Plan, slug: &str) -> Result<Staged, String> {
        deploycmd::stage(self.api.as_ref(), plan, slug)
    }

    /// Deploy what was staged, approving what it needs as us, with its text as written.
    pub(crate) fn commit(&self, plan: &agent::Plan, staged: Staged, text: &str) -> Result<(), String> {
        deploycmd::commit(self.api.as_ref(), plan, staged, crate::mcp::saved_document(plan, text))
    }

    /// Save a flow without deploying it, as the flows page lists it.
    pub(crate) fn save(&self, name: &str, plan: &agent::Plan, text: &str) -> Result<(), String> {
        self.post("/flows", json!({ "name": name, "document": crate::mcp::saved_document(plan, text) })).map(|_| ())
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

    /// Change a grant: revoke items, or grant revoked ones back.
    pub(crate) fn change_grant(&self, slug: &str, revoke: &[String], grant: &[String]) -> Result<(), String> {
        self.post(&format!("/deployments/{}/grant", seg(slug)), json!({ "revoke": revoke, "grant": grant })).map(|_| ())
    }

    /// Set a secret to a value typed here (never shown again, here or anywhere).
    pub(crate) fn set_secret(&self, slug: &str, name: &str, value: &str) -> Result<Value, String> {
        self.post(&format!("/deployments/{}/secrets/{}", seg(slug), seg(name)), json!({ "value": value }))
    }

    pub(crate) fn remove_secret(&self, slug: &str, name: &str) -> Result<(), String> {
        self.delete(&format!("/deployments/{}/secrets/{}", seg(slug), seg(name))).map(|_| ())
    }

    /// Its state's cells, as the server keeps them (`…/state`): key to value.
    pub(crate) fn state(&self, slug: &str) -> Result<Value, String> {
        self.get(&format!("/deployments/{}/state", seg(slug))).map(|s| s["state"].clone())
    }

    /// A collection's first items: (key, value).
    pub(crate) fn collection(&self, slug: &str, name: &str) -> Result<(Vec<(String, Value)>, bool), String> {
        let c = self.get(&format!("/deployments/{}/collections/{}?limit=100", seg(slug), seg(name)))?;
        let items = c["items"].as_array().into_iter().flatten().filter_map(|i| Some((i.get(0)?.as_str()?.to_string(), i.get(1).cloned().unwrap_or(Value::Null)))).collect();
        Ok((items, !c["next"].is_null()))
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
