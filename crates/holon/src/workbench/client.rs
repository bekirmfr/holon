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
}

/// One deployment's pages: what it may do, its runs and timeline.
pub(crate) struct Detail {
    pub(crate) slug: String,
    pub(crate) entry: Value,
    pub(crate) manifest: Value,
    /// Its newest runs, oldest first.
    pub(crate) runs: Vec<Value>,
    pub(crate) timeline: Vec<String>,
    /// The names of the secrets set for it.
    pub(crate) secrets: Vec<String>,
    /// Its text as deployed, or why it can't be shown.
    pub(crate) text: Result<String, String>,
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
        Client { api, me: None, deployments: vec![], flows: vec![] }
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

    /// Read the deployments and saved flows again.
    pub(crate) fn refresh(&mut self) -> Result<(), String> {
        let deployments = list(&self.get("/deployments")?, "deployments");
        let flows = self.get("/flows")?.as_array().cloned().unwrap_or_default();
        if self.me.is_none() {
            self.me = self.get("/auth/me").ok().and_then(|m| m.get("name").and_then(Value::as_str).map(str::to_string));
        }
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
        let versions = crate::history::Versions::of(&self.get(&format!("{path}/versions"))?);
        let timeline = crate::history::timeline(&runs, &events, &versions, false);
        let secrets = self.get(&format!("{path}/secrets")).map(|s| strings(&s, "names")).unwrap_or_default();
        let text = self.get(&format!("{path}/document")).and_then(|d| agent::text_of(slug, &d));
        Ok(Detail { slug: slug.to_string(), entry, manifest, runs, timeline, secrets, text })
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

    /// Start, pause or stop a deployment.
    pub(crate) fn lifecycle(&self, slug: &str, action: &str) -> Result<(), String> {
        self.post(&format!("/deployments/{}/{action}", seg(slug)), json!({})).map(|_| ())
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
}
