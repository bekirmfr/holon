//! `holon mcp`: the tools an agent uses to write, deploy, call and understand flows, as a Model
//! Context Protocol server on stdin and stdout (docs/L4s/agent-surface-L4.md). It speaks the
//! language's text and answers in sentences; what it computes is holon-author's (`agent.rs`), and
//! what it asks a server is what the editor asks. The tools never approve: a deploy that needs a
//! person's approval is saved for them, and says so. It asks as an agent (`X-Holon-Role`), so the
//! server holds it to that whatever sign-in it has (access.rs).

use std::io::{BufRead, Read, Write};
use std::time::Duration;

use holon_author::agent;
use serde_json::{json, Value};

use crate::local;

/// The protocol versions this server speaks, newest last: it answers with the client's when it
/// knows it, else its newest.
const VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

const USAGE: &str = "usage: holon mcp [--server PORT|URL]
       holon mcp setup [HOST…|--all] [--server PORT|URL] [--project DIR] [--print] [--remove]
       holon mcp hosts [add NAME --config PATH --like HOST | remove NAME]

An MCP server on stdin and stdout, for an agent's host to start (Claude Code, an IDE): tools to
look up the language, check a flow, deploy it, call it, list its runs and explain one. It reaches
holon serve as holon export does: --server (or HOLON_SERVER) if given, else the one server you
run on this machine, signed in as you. Its tools never approve what a flow may do: a deploy that
needs approval is saved for a person to deploy from the editor. It asks as an agent, so the server
holds it to proposing too; for an agent with a shell, give it an API token with role agent and
--server (setup --server keeps the token; HOLON_TOKEN gives one).

`holon mcp setup HOST` adds it to a host's settings (Claude Code, Claude Desktop, Cursor, VS Code,
Codex, Zed and more: `holon mcp hosts` lists them, and adds your own). `--help` on either says more.";

const INSTRUCTIONS: &str = "Holon runs small services (flows) written in its own language, and \
shows what each may do before it runs. Call `reference` first for the language and its leaves, \
`check` a text before deploying it, then `deploy`, `call` and `explain`; `deployments` and `flows` \
show what is already there, with its text, and `save` keeps a flow without deploying it. A first \
deploy, and any version that needs a new permission, waits for a person to approve it in the \
editor: tell the user what it asks for and where, and deploy again once they have.";

// ── reaching a server ───────────────────────────────────────────────────────────────────────

/// A request body: JSON, or raw bytes (a unit's preimage).
pub(crate) enum Body {
    Json(Value),
    Bytes(Vec<u8>),
}

/// How the tools reach a server: over HTTP to `holon serve`, or, in tests, its router in-process.
pub(crate) trait Api {
    /// Where it is, for sentences.
    fn server(&self) -> String;
    /// Send a request; its status and body.
    fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Vec<u8>), String>;
}

struct Http {
    server: String,
    token: Option<String>,
}

impl Api for Http {
    fn server(&self) -> String {
        self.server.clone()
    }
    fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Vec<u8>), String> {
        let url = format!("{}{path}", self.server);
        // As an agent, whoever's sign-in it holds (access.rs): the server keeps it to proposing.
        let mut request = ureq::request(method, &url).timeout(Duration::from_secs(60)).set("X-Holon-Role", "agent");
        if let Some(token) = &self.token {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        let result = match body {
            None => request.call(),
            Some(Body::Json(v)) => request.set("Content-Type", "application/json").send_string(&v.to_string()),
            Some(Body::Bytes(b)) => request.set("Content-Type", "application/octet-stream").send_bytes(&b),
        };
        let response = match result {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(e) => return Err(format!("can't reach {}: {e}. Is holon serve running? (holon mcp --server takes the port or address it printed)", self.server)),
        };
        let status = response.status();
        let mut bytes = Vec::new();
        response.into_reader().take(64 << 20).read_to_end(&mut bytes).map_err(|e| format!("reading {}'s answer: {e}", self.server))?;
        Ok((status, bytes))
    }
}

/// The JSON a request answered, or why not, in the server's own words.
fn json_of(api: &dyn Api, method: &str, path: &str, body: Option<Body>) -> Result<Value, String> {
    let (status, bytes) = api.send(method, path, body)?;
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok(value);
    }
    let why = value.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| String::from_utf8_lossy(&bytes).trim().to_string());
    Err(match status {
        401 => format!("{} wants a sign-in ({why}): for a server elsewhere, set HOLON_TOKEN to an API token", api.server()),
        403 => format!("{} refused ({why})", api.server()),
        _ => format!("{} answered {status}: {why}", api.server()),
    })
}

/// A path segment, escaped.
fn seg(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b => format!("%{b:02X}"),
        })
        .collect()
}

// ── the tools ───────────────────────────────────────────────────────────────────────────────

/// The tools, each with what it takes, as the protocol lists them.
fn tool_list() -> Value {
    let text = json!({ "type": "string", "description": "the flow, in the Holon language: a whole file, starting with `flow <name>`" });
    let slug = json!({ "type": "string", "description": "the deployment's name (its slug): the flow's name unless deployed under another" });
    json!([
        {
            "name": "reference",
            "title": "Holon language reference",
            "description": "The Holon language on one page, every leaf (building block) with its inputs, outputs and params, and the sample flows. With a topic (a leaf's, template's or sample's name), that one in full. Read it before writing a flow.",
            "inputSchema": { "type": "object", "properties": { "topic": { "type": "string", "description": "a leaf, template or sample name; leave out for everything" } } },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "check",
            "title": "Check a flow",
            "description": "Compile a flow without deploying it: its routes, the unit (version) it deploys as, and what it may do (the permissions it will ask for). A mistake comes back with its line and column and a caret under it.",
            "inputSchema": { "type": "object", "properties": { "text": text }, "required": ["text"] },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "save",
            "title": "Save a flow",
            "description": "Save a flow on the Holon server without deploying it: it appears on the editor's flows page, with its text as written, for a person to open, and `deploy` deploys it later. A flow someone else saved under that name is never replaced.",
            "inputSchema": { "type": "object", "properties": { "text": text, "name": { "type": "string", "description": "the name to save it under: the flow's name unless another is given" } }, "required": ["text"] },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
        },
        {
            "name": "deploy",
            "title": "Deploy a flow",
            "description": "Deploy a flow to the Holon server. It never approves anything: when the flow may do something not yet approved (a first deploy always is), it is saved for a person, and the answer says what they must approve and where. Once they have deployed it, a new version that needs nothing more deploys from here at once.",
            "inputSchema": { "type": "object", "properties": { "text": text, "slug": slug }, "required": ["text"] },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
        },
        {
            "name": "call",
            "title": "Call a deployed flow",
            "description": "Send one HTTP request to a deployed flow's route, exactly as on its own port (it is logged as a run), and answer its response and the run's number for explain.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": slug,
                    "method": { "type": "string", "description": "GET, POST, PUT, PATCH, DELETE…" },
                    "path": { "type": "string", "description": "the path and query, e.g. /notes/1?full=true" },
                    "headers": { "type": "object", "additionalProperties": { "type": "string" } },
                    "body": { "description": "text as is; an object or array is sent as JSON" }
                },
                "required": ["slug", "method", "path"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true }
        },
        {
            "name": "run",
            "title": "Run a flow's action",
            "description": "Run one of a deployed flow's actions (an `action reset (input) { … }` block) by its name, with what to send it (it is logged as a run), and answer what it answered and the run's number for explain. An agent runs only the actions a person lets agents run, in the flow's deploy settings.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": slug,
                    "action": { "type": "string", "description": "the action's name: reset, for `action reset (input)`" },
                    "input": { "description": "what to send it, as JSON (the block's input); leave out for {}" }
                },
                "required": ["slug", "action"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true }
        },
        {
            "name": "deployments",
            "title": "List what is deployed",
            "description": "What is deployed, one line each (state, version, port, runs, who deployed it), what each waits on a person for, and the flows saved but never deployed. With a slug, that one in full: its routes, what it may do, what its grant doesn't cover, the secrets it reads and which are set, what it waits for, and its text as deployed, even when it has never run.",
            "inputSchema": { "type": "object", "properties": { "slug": { "type": "string", "description": "one deployment's name; leave out for all" } } },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "flows",
            "title": "List the saved flows",
            "description": "The flows saved on the server (by a person in the editor, or by an agent), one line each: who saved it, when, and whether it is deployed. With a name, that flow's text, to read or revise and deploy again.",
            "inputSchema": { "type": "object", "properties": { "name": { "type": "string", "description": "one saved flow's name; leave out for all" } } },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "runs",
            "title": "List a flow's runs",
            "description": "A deployed flow's latest runs, one line each: what started it, its answer, how long it took, and why it failed.",
            "inputSchema": { "type": "object", "properties": { "slug": slug, "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "how many, newest last (default 10)" } }, "required": ["slug"] },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "explain",
            "title": "Explain a run",
            "description": "One recorded run as the program: the flow's text with the value each statement had, where a missing value came from, what it answered, the effects it performed and the calls it made. The latest run unless one is named.",
            "inputSchema": { "type": "object", "properties": { "slug": slug, "run": { "type": "integer", "description": "the run's number (from call or runs); the latest when left out" } }, "required": ["slug"] },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        }
    ])
}

/// How the tools get a server, when one is needed.
pub(crate) type Connect<'a> = dyn Fn() -> Result<Box<dyn Api + 'a>, String> + 'a;

fn arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name).and_then(Value::as_str).ok_or_else(|| format!("`{name}` is missing"))
}

/// Run the tool `name`: its answer, or why it failed (both sentences).
pub(crate) fn call_tool(name: &str, args: &Value, connect: &Connect) -> Result<String, String> {
    match name {
        "reference" => agent::reference(args.get("topic").and_then(Value::as_str)),
        "check" => {
            let r = holon_author::commands::check("text", arg(args, "text")?);
            let out = r.out.join("\n");
            if r.code == 0 { Ok(out) } else { Err([out, r.err.join("\n")].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("\n")) }
        }
        "save" => save(&*connect()?, arg(args, "text")?, args.get("name").and_then(Value::as_str)),
        "deploy" => deploy(&*connect()?, arg(args, "text")?, args.get("slug").and_then(Value::as_str)),
        "call" => call(&*connect()?, args),
        "run" => run_action(&*connect()?, arg(args, "slug")?, arg(args, "action")?, args.get("input").cloned().unwrap_or_else(|| json!({}))),
        "deployments" => deployments(&*connect()?, args.get("slug").and_then(Value::as_str).filter(|s| !s.trim().is_empty())),
        "flows" => flows(&*connect()?, args.get("name").and_then(Value::as_str).filter(|s| !s.trim().is_empty())),
        "runs" => runs(&*connect()?, arg(args, "slug")?, args.get("limit").and_then(Value::as_u64).unwrap_or(10).clamp(1, 200)),
        "explain" => explain(&*connect()?, arg(args, "slug")?, args.get("run").and_then(Value::as_u64)),
        other => Err(format!("there is no tool \"{other}\"")),
    }
}

fn deploy(api: &dyn Api, text: &str, slug: Option<&str>) -> Result<String, String> {
    let plan = agent::plan(text)?;
    let slug = slug.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&plan.name).to_string();
    for unit in &plan.units {
        json_of(api, "POST", "/unit", Some(Body::Bytes(unit.clone())))?;
    }
    let mut body = json!({ "slug": slug, "unit": plan.unit });
    if let Some(r) = &plan.requirements {
        body["requirements"] = r.clone();
    }
    // With its schedule, so the preview asks what the deploy will: a clock flow runs on the clock.
    if let Some(i) = plan.interval_seconds {
        body["interval_seconds"] = json!(i);
    }
    let preview = json_of(api, "POST", "/deploy/preview", Some(Body::Json(body.clone())))?;
    if let Some(why) = preview.get("refused").and_then(Value::as_str) {
        return Err(format!("this server won't run it: {why}"));
    }
    let list = |k: &str| -> Vec<String> { preview.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default() };
    let (needs, missing) = (list("needs_approval"), list("missing_secrets"));
    let described = |items: &[String]| items.iter().map(|i| format!("  - {}", agent::describe(i))).collect::<Vec<_>>().join("\n");
    let secrets_note = if missing.is_empty() {
        String::new()
    } else {
        format!("\nSecrets not set yet (an admin sets them in the editor, under the deployment's ⚙): {}. Until then, a run that reads one gets nothing.", missing.join(", "))
    };
    if !needs.is_empty() {
        save_for_approval(api, &slug, &saved_document(&plan, text))?;
        return Ok(format!(
            "Not deployed yet: a person has to approve what \"{slug}\" may do. It asks to:\n{}\n\nIt is saved as the flow \"{slug}\" on {}: they open the editor there, open \"{slug}\" from the flows page, and deploy it, which asks them to approve exactly these. Tell them, and deploy again from here once they have: a version that needs nothing more deploys at once.{secrets_note}",
            described(&needs),
            api.server()
        ));
    }
    body["acknowledge"] = json!([]);
    body["document"] = plan.document.clone();
    if let Some(b) = &plan.browsers {
        // The plan keeps the origins on one line (`https://a.example *`); the deploy request
        // takes them one by one, as the editor sends them.
        body["browsers"] = json!(b.split_whitespace().collect::<Vec<_>>());
    }
    json_of(api, "POST", "/deploy", Some(Body::Json(body)))?;
    let entry = json_of(api, "GET", "/deployments", None)?
        .get("deployments")
        .and_then(Value::as_array)
        .and_then(|all| all.iter().find(|d| d.get("slug").and_then(Value::as_str) == Some(slug.as_str())).cloned())
        .unwrap_or(Value::Null);
    let mut out = vec![format!("Deployed \"{slug}\", version {}.", &plan.unit[..12])];
    out.push(if plan.may.is_empty() { "It only computes.".into() } else { format!("It may:\n{}", described(&plan.may)) });
    if !plan.routes.is_empty() {
        let port = entry.get("port").and_then(Value::as_u64).map(|p| p.to_string()).or(plan.port.map(|p| p.to_string())).unwrap_or_else(|| "?".into());
        out.push(format!("Its routes, answering on port {port}:\n{}", plan.routes.iter().map(|r| format!("  {r}")).collect::<Vec<_>>().join("\n")));
    } else if let Some(i) = plan.interval_seconds {
        out.push(format!("It runs every {i} s."));
    }
    if let Some(e) = entry.get("listen_error").and_then(Value::as_str) {
        out.push(format!("But its port can't be opened: {e}"));
    }
    if let Some(status) = entry.get("status").and_then(Value::as_str).filter(|s| *s != "running") {
        out.push(format!("It is {status}: a person starts it from the editor's deployed page."));
    }
    out.push(format!("Call it with `call`, and see what a run did with `explain`.{secrets_note}"));
    Ok(out.join("\n"))
}

/// Save the flow for a person to deploy, under `slug`, unless that would replace a flow someone
/// else saved.
fn save_for_approval(api: &dyn Api, slug: &str, document: &Value) -> Result<(), String> {
    match save_flow(api, slug, document)? {
        None => Ok(()),
        Some(by) => Err(format!("not deployed: it needs a person's approval, and a flow named \"{slug}\" saved by {by} is already on {}; deploy under another slug so theirs isn't replaced", api.server())),
    }
}

/// Save `document` as the flow `name`; or, when a flow by that name is someone else's, leave it
/// and answer whose it is.
fn save_flow(api: &dyn Api, name: &str, document: &Value) -> Result<Option<String>, String> {
    let me = json_of(api, "GET", "/auth/me", None).ok().and_then(|m| m.get("name").and_then(Value::as_str).map(str::to_string));
    let (status, bytes) = api.send("GET", &format!("/flows/{}", seg(name)), None)?;
    if status == 200 {
        let saved: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let by = saved.get("saved_by").and_then(Value::as_str);
        if by.is_some() && by != me.as_deref() {
            return Ok(Some(by.unwrap_or("someone").to_string()));
        }
    }
    json_of(api, "POST", "/flows", Some(Body::Json(json!({ "name": name, "document": document }))))?;
    Ok(None)
}

/// The document a flow is saved with: the compiled canvas, and the text as written, comments
/// and all, so the person who opens it reads what the agent wrote (the editor's text pane).
fn saved_document(plan: &agent::Plan, text: &str) -> Value {
    let mut document = plan.document.clone();
    document["source"] = json!(text);
    document
}

fn save(api: &dyn Api, text: &str, name: Option<&str>) -> Result<String, String> {
    let plan = agent::plan(text)?;
    let name = name.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&plan.name).to_string();
    if let Some(by) = save_flow(api, &name, &saved_document(&plan, text))? {
        return Err(format!("not saved: a flow named \"{name}\" saved by {by} is already on {}; save it under another name so theirs isn't replaced", api.server()));
    }
    let may = if plan.may.is_empty() { "It only computes.".to_string() } else { format!("When deployed, it may:\n{}", plan.may.iter().map(|i| format!("  - {}", agent::describe(i))).collect::<Vec<_>>().join("\n")) };
    Ok(format!(
        "Saved \"{name}\" on {}, not deployed: it is on the editor's flows page, where a person can open it, and `deploy` deploys it (a first deploy, or one that needs a new permission, waits for them).\n{may}",
        api.server()
    ))
}

/// When, as a person reads it.
fn when(v: &Value) -> String {
    v.as_u64().map(agent::utc).unwrap_or_else(|| "?".into())
}

/// The things a deployment waits on a person for, each said as what they do.
fn waits_for(entry: &Value, saved: Option<&Value>) -> Vec<String> {
    let slug = entry.get("slug").and_then(Value::as_str).unwrap_or("?");
    let mut waits = Vec::new();
    if let Some(a) = entry.get("attention").and_then(Value::as_str) {
        waits.push(format!("to deploy it again, from the editor: {a}"));
    }
    if let Some(s @ ("paused" | "stopped")) = entry.get("status").and_then(Value::as_str) {
        waits.push(format!("to start it: it is {s} (the editor's deployed page)"));
    }
    if let Some(e) = entry.get("listen_error").and_then(Value::as_str) {
        waits.push(format!("to free its port: {e}"));
    }
    let deployed_at = entry.get("deployed_at_epoch_seconds").and_then(Value::as_u64).unwrap_or(0);
    if let Some(f) = saved.filter(|f| f.get("saved_at_epoch_seconds").and_then(Value::as_u64).unwrap_or(0) > deployed_at) {
        let by = f.get("saved_by").and_then(Value::as_str).unwrap_or("someone");
        waits.push(format!("to review a version saved since by {by}, at {}: open \"{slug}\" from the flows page, and deploy it", when(&f["saved_at_epoch_seconds"])));
    }
    waits
}

fn list_of(v: Value, key: Option<&str>) -> Vec<Value> {
    let v = match key {
        Some(k) => v.get(k).cloned().unwrap_or(Value::Null),
        None => v,
    };
    v.as_array().cloned().unwrap_or_default()
}

/// A deployment in one line: its state, version, port, runs and who deployed it.
fn deployment_line(d: &Value) -> String {
    let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("?");
    let n = |k: &str| d.get(k).and_then(Value::as_u64).unwrap_or(0);
    let unit = s("unit");
    let mut line = format!("{}: {}, version {}", s("slug"), s("status"), &unit[..unit.len().min(12)]);
    if let Some(p) = d.get("port").and_then(Value::as_u64) {
        line.push_str(&format!(", port {p}"));
    } else if let Some(i) = d.get("interval_seconds").and_then(Value::as_u64) {
        line.push_str(&format!(", every {i} s"));
    }
    let runs = n("trigger_count");
    line.push_str(&format!("; {runs} run{}, {} failed", if runs == 1 { "" } else { "s" }, n("error_count")));
    if n("waiting") > 0 {
        line.push_str(&format!(", {} waiting for an event", n("waiting")));
    }
    let by = d.get("deployed_by").and_then(Value::as_str).map(|b| format!(" by {b}")).unwrap_or_default();
    line.push_str(&format!("; deployed{by} at {}", when(&d["deployed_at_epoch_seconds"])));
    line
}

fn deployments(api: &dyn Api, slug: Option<&str>) -> Result<String, String> {
    let all = list_of(json_of(api, "GET", "/deployments", None)?, Some("deployments"));
    let flows = list_of(json_of(api, "GET", "/flows", None)?, None);
    let saved = |slug: &str| flows.iter().find(|f| f.get("name").and_then(Value::as_str) == Some(slug));
    let deployed = |slug: &str| all.iter().any(|d| d.get("slug").and_then(Value::as_str) == Some(slug));
    let Some(slug) = slug else {
        let mut out = Vec::new();
        if all.is_empty() {
            out.push(format!("Nothing is deployed on {}.", api.server()));
        } else {
            out.push(format!("Deployed on {}:", api.server()));
        }
        for d in &all {
            out.push(format!("  {}", deployment_line(d)));
            for w in waits_for(d, d.get("slug").and_then(Value::as_str).and_then(saved)) {
                out.push(format!("    waits for a person {w}"));
            }
        }
        let waiting: Vec<&Value> = flows.iter().filter(|f| f.get("name").and_then(Value::as_str).is_some_and(|n| !deployed(n))).collect();
        if !waiting.is_empty() {
            out.push("Saved, never deployed (a person deploys one from the flows page; `flows` shows its text):".into());
            for f in waiting {
                let by = f.get("saved_by").and_then(Value::as_str).map(|b| format!(" by {b}")).unwrap_or_default();
                out.push(format!("  {}: saved{by} at {}", f["name"].as_str().unwrap_or("?"), when(&f["saved_at_epoch_seconds"])));
            }
        }
        return Ok(out.join("\n"));
    };
    let Some(d) = all.iter().find(|d| d.get("slug").and_then(Value::as_str) == Some(slug)) else {
        return match saved(slug) {
            Some(f) => Ok(format!(
                "\"{slug}\" isn't deployed. It is saved{} at {}, for a person to deploy from the flows page; `flows` with its name shows its text.",
                f.get("saved_by").and_then(Value::as_str).map(|b| format!(" by {b}")).unwrap_or_default(),
                when(&f["saved_at_epoch_seconds"])
            )),
            None => Err(format!("nothing named \"{slug}\" is deployed or saved on {}", api.server())),
        };
    };
    let path = format!("/deployments/{}", seg(slug));
    let manifest = json_of(api, "GET", &format!("{path}/manifest"), None)?;
    let secrets = json_of(api, "GET", &format!("{path}/secrets"), None).map(|s| list_of(s, Some("names"))).unwrap_or_default();
    let text = agent::text_of(slug, &json_of(api, "GET", &format!("{path}/document"), None)?);
    let strings = |v: &Value, k: &str| -> Vec<String> { v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|i| i.as_str().map(str::to_string)).collect()).unwrap_or_default() };
    let mut out = vec![format!("On {}: {}", api.server(), deployment_line(d))];
    if let Ok(plan) = text.as_deref().map_err(String::clone).and_then(agent::plan)
        && !plan.routes.is_empty()
    {
        out.push(format!("Its routes: {}", plan.routes.join(", ")));
    }
    let items = strings(&manifest, "items");
    out.push(if items.is_empty() { "It only computes.".into() } else { format!("It may:\n{}", items.iter().map(|i| format!("  - {}", agent::describe(i))).collect::<Vec<_>>().join("\n")) });
    let not_granted = strings(&manifest, "not_granted");
    if !not_granted.is_empty() {
        out.push(format!("Its grant doesn't cover:\n{}", not_granted.iter().map(|i| format!("  - {}", agent::describe(i))).collect::<Vec<_>>().join("\n")));
    }
    let reads: Vec<String> = strings(&manifest, "capabilities").iter().filter_map(|c| c.strip_prefix("SecretRead(\"")?.strip_suffix("\")").map(str::to_string)).collect();
    if !reads.is_empty() {
        let named: Vec<String> = reads.iter().map(|r| if secrets.iter().any(|s| s.as_str() == Some(r)) { format!("{r} (set)") } else { format!("{r} (not set: an admin sets it under the deployment's ⚙)") }).collect();
        out.push(format!("The secrets it reads: {}", named.join(", ")));
    }
    let waits = waits_for(d, saved(slug));
    out.push(if waits.is_empty() { "Nothing waits for a person.".into() } else { format!("It waits for a person:\n{}", waits.iter().map(|w| format!("  - {w}")).collect::<Vec<_>>().join("\n")) });
    match text {
        Ok(t) => out.push(format!("\nIts text, as deployed:\n\n{}", t.trim_end())),
        Err(e) => out.push(format!("\nIts text can't be shown: {e}")),
    }
    Ok(out.join("\n"))
}

fn flows(api: &dyn Api, name: Option<&str>) -> Result<String, String> {
    let deployed: Vec<String> = list_of(json_of(api, "GET", "/deployments", None)?, Some("deployments")).iter().filter_map(|d| d.get("slug").and_then(Value::as_str).map(str::to_string)).collect();
    let saved_line = |f: &Value| {
        let n = f.get("name").and_then(Value::as_str).unwrap_or("?");
        let by = f.get("saved_by").and_then(Value::as_str).map(|b| format!(" by {b}")).unwrap_or_default();
        let state = if deployed.iter().any(|d| d == n) { "; deployed under that name" } else { "; not deployed" };
        format!("{n}: saved{by} at {}{state}", when(&f["saved_at_epoch_seconds"]))
    };
    let Some(name) = name else {
        let all = list_of(json_of(api, "GET", "/flows", None)?, None);
        if all.is_empty() {
            return Ok(format!("No flows are saved on {}. The samples are in `reference`.", api.server()));
        }
        let mut out = vec![format!("Saved on {} (`flows` with a name shows one's text; the samples are in `reference`):", api.server())];
        out.extend(all.iter().map(|f| format!("  {}", saved_line(f))));
        return Ok(out.join("\n"));
    };
    let record = json_of(api, "GET", &format!("/flows/{}", seg(name)), None)?;
    let text = agent::text_of(name, record.get("document").unwrap_or(&Value::Null))?;
    Ok(format!("{}\n\n{}", saved_line(&record), text.trim_end()))
}

fn call(api: &dyn Api, args: &Value) -> Result<String, String> {
    let slug = arg(args, "slug")?;
    let method = arg(args, "method")?.to_ascii_uppercase();
    let path = arg(args, "path")?;
    let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
    let mut headers: Vec<(String, String)> =
        args.get("headers").and_then(Value::as_object).map(|h| h.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect()).unwrap_or_default();
    let body = match args.get("body") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => {
            if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                headers.push(("content-type".into(), "application/json".into()));
            }
            v.to_string()
        }
    };
    let log = format!("/deployments/{}/log", seg(slug));
    let next = json_of(api, "GET", &format!("{log}?limit=0"), None)?.get("next").and_then(Value::as_u64).unwrap_or(0);
    let answer = json_of(api, "POST", &format!("/deployments/{}/request", seg(slug)), Some(Body::Json(json!({ "method": method, "url": path, "headers": headers, "body": body }))))?;
    let status = answer.get("status").and_then(Value::as_u64).unwrap_or(0);
    let mut out = vec![format!("{method} {path} → {status}")];
    for h in answer.get("headers").and_then(Value::as_array).into_iter().flatten() {
        if let (Some(k), Some(v)) = (h.get(0).and_then(Value::as_str), h.get(1).and_then(Value::as_str))
            && (k.eq_ignore_ascii_case("content-type") || k.eq_ignore_ascii_case("location"))
        {
            out.push(format!("{k}: {v}"));
        }
    }
    let body = answer.get("body").and_then(Value::as_str).unwrap_or("");
    if !body.is_empty() {
        out.push(String::new());
        out.push(if body.chars().count() > 8000 { format!("{}…", body.chars().take(8000).collect::<String>()) } else { body.to_string() });
    }
    let after = if next == 0 { String::new() } else { format!("after={}&", next - 1) };
    let run = json_of(api, "GET", &format!("{log}?{after}limit=1000"), None)?
        .get("entries")
        .and_then(Value::as_array)
        .and_then(|e| e.iter().find(|e| e.get("method").and_then(Value::as_str) == Some(method.as_str()) && e.get("url").and_then(Value::as_str) == Some(path.as_str())).cloned());
    out.push(String::new());
    match run.as_ref().and_then(|r| r.get("seq")).and_then(Value::as_u64) {
        Some(seq) if run.as_ref().and_then(|r| r.get("run_id")).and_then(Value::as_str).is_some_and(|id| !id.is_empty()) => {
            out.push(format!("This was run {seq}: `explain` with run {seq} shows what each statement did."));
        }
        Some(_) => out.push("No route matched it, so no run was made.".into()),
        None => out.push("Its run isn't in the log.".into()),
    }
    Ok(out.join("\n"))
}

/// Run `slug`'s action `name` with `input`: what it answered, and its run.
fn run_action(api: &dyn Api, slug: &str, name: &str, input: Value) -> Result<String, String> {
    let ran = json_of(api, "POST", &format!("/trigger/{}/{}", seg(slug), seg(name)), Some(Body::Json(input)))?;
    let seq = ran.get("seq").and_then(Value::as_u64).unwrap_or(0);
    if ran.get("waiting").and_then(Value::as_bool) == Some(true) {
        return Ok(format!("action {name} is waiting for an event: run {seq}, which `explain` shows."));
    }
    let answer = ran.get("answer").cloned().unwrap_or(Value::Null);
    Ok(format!("action {name} → {answer}\n\nThis was run {seq}: `explain` with run {seq} shows what each statement did."))
}

fn runs(api: &dyn Api, slug: &str, limit: u64) -> Result<String, String> {
    let log = json_of(api, "GET", &format!("/deployments/{}/log?limit={limit}", seg(slug)), None)?;
    let entries = log.get("entries").and_then(Value::as_array).cloned().unwrap_or_default();
    if entries.is_empty() {
        return Ok(format!("\"{slug}\" has no runs yet."));
    }
    let mut out: Vec<String> = entries.iter().map(agent::run_line).collect();
    if let Some(breaks) = log.get("breaks").and_then(Value::as_array).filter(|b| !b.is_empty()) {
        out.push(format!("The log was changed outside holon before runs {}: their record can't be trusted.", breaks.iter().map(Value::to_string).collect::<Vec<_>>().join(", ")));
    }
    Ok(out.join("\n"))
}

fn explain(api: &dyn Api, slug: &str, run: Option<u64>) -> Result<String, String> {
    let log = format!("/deployments/{}/log", seg(slug));
    let entry = match run {
        None => json_of(api, "GET", &format!("{log}?limit=1"), None)?.get("entries").and_then(Value::as_array).and_then(|e| e.last().cloned()),
        Some(seq) => {
            let after = if seq == 0 { String::new() } else { format!("after={}&", seq - 1) };
            json_of(api, "GET", &format!("{log}?{after}limit=100000"), None)?.get("entries").and_then(Value::as_array).and_then(|e| e.iter().find(|e| e.get("seq").and_then(Value::as_u64) == Some(seq)).cloned())
        }
    };
    let Some(entry) = entry else {
        return Err(match run {
            Some(seq) => format!("\"{slug}\" has no run {seq} (it may be older than the log keeps)"),
            None => format!("\"{slug}\" has no runs yet"),
        });
    };
    let unit = entry.get("unit").and_then(Value::as_str).filter(|u| !u.is_empty());
    let path = match unit {
        Some(u) => format!("/deployments/{}/document?unit={u}", seg(slug)),
        None => format!("/deployments/{}/document", seg(slug)),
    };
    let document = json_of(api, "GET", &path, None)?;
    agent::explain(slug, &document, &entry)
}

// ── the protocol ────────────────────────────────────────────────────────────────────────────

/// The answer to one JSON-RPC message; none for a notification.
pub(crate) fn handle(message: &Value, connect: &Connect) -> Option<Value> {
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str);
    let reply = |result: Result<Value, (i64, String)>| {
        let id = id.clone().unwrap_or(Value::Null);
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, m)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": m } }),
        })
    };
    let Some(method) = method else {
        // A response to something this server never asks, or not a request at all.
        return if id.is_some() && message.get("result").is_none() && message.get("error").is_none() { reply(Err((-32600, "not a request".into()))) } else { None };
    };
    // A notification: no answer.
    id.as_ref()?;
    match method {
        "initialize" => {
            let asked = message.pointer("/params/protocolVersion").and_then(Value::as_str);
            let version = asked.filter(|v| VERSIONS.contains(v)).unwrap_or(VERSIONS[VERSIONS.len() - 1]);
            reply(Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "holon", "title": "Holon", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            })))
        }
        "ping" => reply(Ok(json!({}))),
        "tools/list" => reply(Ok(json!({ "tools": tool_list() }))),
        "tools/call" => {
            let Some(name) = message.pointer("/params/name").and_then(Value::as_str) else {
                return reply(Err((-32602, "tools/call needs params.name".into())));
            };
            if !tool_list().as_array().unwrap().iter().any(|t| t["name"] == name) {
                return reply(Err((-32602, format!("there is no tool \"{name}\""))));
            }
            let args = message.pointer("/params/arguments").cloned().unwrap_or(json!({}));
            let (text, failed) = match call_tool(name, &args, connect) {
                Ok(t) => (t, false),
                Err(e) => (e, true),
            };
            reply(Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": failed })))
        }
        other => reply(Err((-32601, format!("no method \"{other}\"")))),
    }
}

/// Serve the protocol on `input` and `output` until the input ends.
pub(crate) fn serve(input: impl BufRead, mut output: impl Write, connect: &Connect) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(message) if message.is_object() => handle(&message, connect),
            Ok(_) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32600, "message": "one JSON-RPC message per line, not a batch" } })),
            Err(e) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("not JSON: {e}") } })),
        };
        if let Some(a) = answer {
            writeln!(output, "{a}")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// `holon mcp`.
pub(crate) fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("setup") => return crate::mcphosts::setup(&args[1..]),
        Some("hosts") => return crate::mcphosts::hosts(&args[1..]),
        _ => {}
    }
    let mut server: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--server" => match it.next() {
                Some(s) => server = Some(s.clone()),
                None => {
                    eprintln!("{USAGE}");
                    return 2;
                }
            },
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                return 0;
            }
            _ => {
                eprintln!("holon mcp: {a}?\n\n{USAGE}");
                return 2;
            }
        }
    }
    // Found again for each tool that needs it, so a server started after the agent is reached.
    let connect = || -> Result<Box<dyn Api>, String> {
        let t = local::target(server.as_deref())?;
        Ok(Box::new(Http { server: t.server, token: t.token }))
    };
    eprintln!("holon mcp: ready on stdin and stdout");
    match serve(std::io::stdin().lock(), std::io::stdout().lock(), &connect) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("holon mcp: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{body_bytes, req, test_state};
    use crate::{route, AppState};

    /// The router in-process, as the tools' server.
    struct InProcess<'a>(&'a AppState);

    impl Api for InProcess<'_> {
        fn server(&self) -> String {
            "the test server".into()
        }
        fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Vec<u8>), String> {
            let bytes = match body {
                None => vec![],
                Some(Body::Json(v)) => serde_json::to_vec(&v).unwrap(),
                // `req` sends bytes not starting with `{` as octet-stream.
                Some(Body::Bytes(b)) => b,
            };
            let response = route(self.0, &req(method, path, bytes));
            Ok((response.status_code, body_bytes(response)))
        }
    }

    fn tool(state: &AppState, name: &str, args: Value) -> Result<String, String> {
        let connect = || -> Result<Box<dyn Api + '_>, String> { Ok(Box::new(InProcess(state))) };
        call_tool(name, &args, &connect)
    }

    /// A server whose answers are given: for what the in-process router can't be made to say.
    struct Given(Vec<(&'static str, &'static str, u16, Value)>);

    impl Api for Given {
        fn server(&self) -> String {
            "the given server".into()
        }
        fn send(&self, method: &str, path: &str, _: Option<Body>) -> Result<(u16, Vec<u8>), String> {
            let (_, _, status, body) = self.0.iter().find(|(m, p, _, _)| *m == method && *p == path).ok_or(format!("unexpected {method} {path}"))?;
            Ok((*status, serde_json::to_vec(body).unwrap()))
        }
    }

    const SLUGS_TEXT: &str = include_str!("../../holon-author/samples/slugs.holon");

    #[test]
    fn a_flow_goes_from_text_to_an_explained_run_and_the_agent_never_approves() {
        let state = test_state();
        let text = SLUGS_TEXT;
        // Checked without a server.
        let checked = tool(&state, "check", json!({ "text": text })).unwrap();
        assert!(checked.contains("POST /slugs") && checked.contains("it may:"), "{checked}");
        // A first deploy waits for a person: saved, not deployed, and it says what they approve.
        let first = tool(&state, "deploy", json!({ "text": text })).unwrap();
        assert!(first.starts_with("Not deployed yet"), "{first}");
        assert!(first.contains("receive requests") && first.contains("\"slugs\""), "{first}");
        let saved = serde_json::from_slice::<Value>(&body_bytes(route(&state, &req("GET", "/flows/slugs", vec![])))).unwrap();
        assert!(saved["document"]["trace"].is_object(), "the saved flow is the document a deploy sends");
        assert!(serde_json::from_slice::<Value>(&body_bytes(route(&state, &req("GET", "/deployments", vec![])))).unwrap()["deployments"].as_array().unwrap().is_empty());
        // A person approves it (the editor's deploy: everything the preview asked).
        let plan = agent::plan(text).unwrap();
        let preview: Value = serde_json::from_slice(&body_bytes(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&json!({ "slug": "slugs", "unit": plan.unit })).unwrap())))).unwrap();
        let approve = json!({ "slug": "slugs", "unit": plan.unit, "acknowledge": preview["needs_approval"], "document": plan.document });
        assert_eq!(route(&state, &req("POST", "/deploy", serde_json::to_vec(&approve).unwrap())).status_code, 200);
        // A new version that needs nothing more deploys from the agent at once.
        let v2 = text.replace("\"unsigned\"", "\"not signed\"");
        let second = tool(&state, "deploy", json!({ "text": v2 })).unwrap();
        assert!(second.starts_with("Deployed \"slugs\""), "{second}");
        assert!(second.contains("POST /slugs"), "{second}");
        // Called: its answer, and its run.
        let called = tool(&state, "call", json!({ "slug": "slugs", "method": "post", "path": "/slugs", "body": { "title": "Hello World", "author": "Ada" } })).unwrap();
        assert!(called.starts_with("POST /slugs → 200"), "{called}");
        assert!(called.contains("hello-world") && called.contains("not signed"), "{called}");
        assert!(called.contains("This was run"), "{called}");
        // One that fails at a missing field, explained: the statement, and where the value went missing.
        let failed = tool(&state, "call", json!({ "slug": "slugs", "method": "POST", "path": "/slugs", "body": { "title": "only a title" } })).unwrap();
        assert!(failed.starts_with("POST /slugs → 400"), "{failed}");
        let listed = tool(&state, "runs", json!({ "slug": "slugs" })).unwrap();
        assert_eq!(listed.lines().count(), 2, "{listed}");
        assert!(listed.lines().nth(1).unwrap().contains("POST /slugs → 400"), "{listed}");
        let why = tool(&state, "explain", json!({ "slug": "slugs" })).unwrap();
        assert!(why.contains("author = slugify(req.json.author)"), "the agent's own names: {why}");
        assert!(why.contains("⇒ nothing: field \"author\" gave nothing"), "{why}");
        assert!(why.contains("⇒ \"only-a-title\""), "{why}");
        assert!(why.contains("⇒ req.json = { title: \"only a title\" }"), "the request, as it arrived: {why}");
        assert!(!why.contains("didn't run"), "the other route is shown without values: {why}");
        let first_run = called.split("This was run ").nth(1).unwrap().split(':').next().unwrap().parse::<u64>().unwrap();
        let ok = tool(&state, "explain", json!({ "slug": "slugs", "run": first_run })).unwrap();
        assert!(ok.contains("⇒ \"ada\"") && ok.contains("answered 200"), "{ok}");
        assert!(ok.contains("performed"), "its writes: {ok}");
    }

    /// A flow's `browsers` line reaches the server as the list its deploy request takes, so a
    /// flow that answers web pages deploys from the agent like any other.
    #[test]
    fn a_flow_that_answers_browsers_deploys_with_them_as_a_list() {
        let state = test_state();
        let text = "flow page-api\nport 8199\nbrowsers https://app.example, *\n\nroute GET /hi (req) {\n  name = req.query.name ?? \"you\"\n  respond \"hi {name}\"\n}\n";
        let first = tool(&state, "deploy", json!({ "text": text })).unwrap();
        assert!(first.starts_with("Not deployed yet"), "{first}");
        let plan = agent::plan(text).unwrap();
        assert_eq!(plan.browsers.as_deref(), Some("https://app.example *"), "the plan keeps them on one line");
        let preview: Value = serde_json::from_slice(&body_bytes(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&json!({ "slug": "page-api", "unit": plan.unit })).unwrap())))).unwrap();
        let approve = json!({ "slug": "page-api", "unit": plan.unit, "acknowledge": preview["needs_approval"], "document": plan.document, "browsers": ["https://app.example", "*"] });
        assert_eq!(route(&state, &req("POST", "/deploy", serde_json::to_vec(&approve).unwrap())).status_code, 200);
        // The next version deploys from the agent, with the origins one by one.
        let v2 = text.replace("\"hi {name}\"", "\"hello {name}\"");
        let second = tool(&state, "deploy", json!({ "text": v2 })).unwrap();
        assert!(second.starts_with("Deployed \"page-api\""), "{second}");
        let deployments: Value = serde_json::from_slice(&body_bytes(route(&state, &req("GET", "/deployments", vec![])))).unwrap();
        let entry = deployments["deployments"].as_array().unwrap().iter().find(|d| d["slug"] == "page-api").cloned().unwrap_or(Value::Null);
        assert_eq!(entry["browsers"], json!(["https://app.example", "*"]), "{entry}");
    }

    #[test]
    fn refusals_and_mistakes_come_back_as_failed_tools_in_sentences() {
        let state = test_state();
        let e = tool(&state, "check", json!({ "text": "flow f\nport 8200\n\nroute GET /a (req) {\n  respond nope\n}\n" })).unwrap_err();
        assert!(e.contains("text:5:11: unknown name \"nope\"") && e.contains('^'), "{e}");
        assert!(tool(&state, "deploy", json!({ "text": "flow f\nport 8200\n\nroute GET /a (req) {\n  respond nope\n}\n" })).is_err());
        assert!(tool(&state, "explain", json!({ "slug": "nobody" })).unwrap_err().contains("404"));
        assert!(tool(&state, "check", json!({})).unwrap_err().contains("`text` is missing"));
    }

    /// The router in-process with a sign-in, sent as `holon mcp` sends it (as an agent) or as the
    /// editor does (as the person).
    struct Signed<'a> {
        state: &'a AppState,
        token: &'a str,
        agent: bool,
    }

    impl Api for Signed<'_> {
        fn server(&self) -> String {
            "the test server".into()
        }
        fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Vec<u8>), String> {
            let bytes = match body {
                None => vec![],
                Some(Body::Json(v)) => serde_json::to_vec(&v).unwrap(),
                Some(Body::Bytes(b)) => b,
            };
            let mut headers: Vec<(String, String)> = req(method, path, bytes.clone()).headers().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            headers.push(("Authorization".into(), format!("Bearer {}", self.token)));
            if self.agent {
                headers.push(("X-Holon-Role".into(), "agent".into()));
            }
            let response = route(self.state, &rouille::Request::fake_http(method, path, headers, bytes));
            Ok((response.status_code, body_bytes(response)))
        }
    }

    /// On a server with accounts, an agent holding an admin's sign-in is still an agent: it
    /// proposes and calls, and the server refuses it the rest, whatever it sends.
    #[test]
    fn an_agent_with_an_admins_sign_in_only_proposes() {
        let mut state = test_state();
        let accounts = std::sync::Arc::new(crate::accounts::Accounts::memory());
        let (ada, _) = accounts.setup(&accounts.setup_code().unwrap(), "ada", "correct horse battery").unwrap();
        state.access = crate::access::Access::new(std::sync::Arc::clone(&accounts), vec![], vec![]);
        let (agent, person) = (Signed { state: &state, token: &ada, agent: true }, Signed { state: &state, token: &ada, agent: false });
        let slug = Some("agent-slugs");
        assert!(deploy(&agent, SLUGS_TEXT, slug).unwrap().starts_with("Not deployed yet"));
        assert_eq!(json_of(&person, "GET", "/flows/agent-slugs", None).unwrap()["saved_by"], "ada (agent)", "proposed under its own name");
        // Approving it itself is refused, as is deploying what needs approval.
        let plan = agent::plan(SLUGS_TEXT).unwrap();
        let ask = |acknowledge: Value| Some(Body::Json(json!({ "slug": "agent-slugs", "unit": plan.unit, "acknowledge": acknowledge, "document": plan.document })));
        let approved = json_of(&agent, "POST", "/deploy", ask(json!(["NetworkIngress"]))).unwrap_err();
        assert!(approved.contains("refused") && approved.contains("an agent doesn't approve"), "{approved}");
        let unasked = json_of(&agent, "POST", "/deploy", ask(json!([]))).unwrap_err();
        assert!(unasked.contains("needs a person's approval"), "{unasked}");
        // The person approves; then the agent's next version deploys, and it calls it.
        let preview = json_of(&person, "POST", "/deploy/preview", Some(Body::Json(json!({ "slug": "agent-slugs", "unit": plan.unit })))).unwrap();
        json_of(&person, "POST", "/deploy", ask(preview["needs_approval"].clone())).unwrap();
        let v2 = SLUGS_TEXT.replace("\"unsigned\"", "\"not signed\"");
        assert!(deploy(&agent, &v2, slug).unwrap().starts_with("Deployed \"agent-slugs\""));
        // Its code changed, so the secret it can run without is withheld until a person grants it back.
        let grant = json_of(&person, "GET", "/deployments/agent-slugs/grant", None).unwrap();
        let receipt = json!("SecretRead(\"receipt_key\")");
        assert!(!grant["grant"]["items"].as_array().unwrap().contains(&receipt), "{grant}");
        assert!(grant["grant"]["revoked"].as_array().unwrap().contains(&receipt), "offered back: {grant}");
        let called = call(&agent, &json!({ "slug": "agent-slugs", "method": "POST", "path": "/slugs", "body": { "title": "Hi", "author": "Ada" } })).unwrap();
        assert!(called.starts_with("POST /slugs → 200"), "{called}");
        let listed = runs(&agent, "agent-slugs", 5).unwrap();
        assert!(listed.contains("sent by ada (agent)"), "the run names who sent it: {listed}");
        // What is a person's stays theirs.
        for (method, path) in [
            ("POST", "/deployments/agent-slugs/grant"),
            ("POST", "/deployments/agent-slugs/stop"),
            ("POST", "/deployments/agent-slugs/run"),
            ("DELETE", "/deployments/agent-slugs"),
            ("POST", "/deployments/agent-slugs/secrets/k"),
            ("DELETE", "/flows/agent-slugs"),
            ("POST", "/trigger/agent-slugs"),
            ("POST", "/permissions/reload"),
            ("GET", "/admin/users"),
        ] {
            assert_eq!(agent.send(method, path, None).unwrap().0, 403, "{method} {path}");
        }
        // The server, too, keeps it from replacing a flow the person saved.
        json_of(&person, "POST", "/flows", Some(Body::Json(json!({ "name": "mine", "document": {} })))).unwrap();
        let replaced = json_of(&agent, "POST", "/flows", Some(Body::Json(json!({ "name": "mine", "document": {} })))).unwrap_err();
        assert!(replaced.contains("saved by ada") && replaced.contains("doesn't replace"), "{replaced}");
        assert_eq!(person.send("POST", "/deployments/agent-slugs/stop", None).unwrap().0, 200);
    }

    /// An agent runs a flow's action only as its grant lets agents: a person asks for it (the
    /// deploy settings' `agents`) and approves `AgentRuns("reset")`; taking it back refuses the
    /// agent again and leaves the flow running. A person runs it either way.
    #[test]
    fn an_agent_runs_an_action_only_as_its_grant_lets_agents() {
        let mut state = test_state();
        let accounts = std::sync::Arc::new(crate::accounts::Accounts::memory());
        let (ada, _) = accounts.setup(&accounts.setup_code().unwrap(), "ada", "correct horse battery").unwrap();
        state.access = crate::access::Access::new(std::sync::Arc::clone(&accounts), vec![], vec![]);
        let (agent, person) = (Signed { state: &state, token: &ada, agent: true }, Signed { state: &state, token: &ada, agent: false });
        let text = include_str!("../../holon-author/samples/ticker.holon").replace("port 8156", "port 18259");
        let plan = agent::plan(&text).unwrap();
        for unit in &plan.units {
            json_of(&person, "POST", "/unit", Some(Body::Bytes(unit.clone()))).unwrap();
        }
        let deploy_with = |agents: Value| -> Result<Value, String> {
            let requirements = json!({ "agents": agents });
            let preview = json_of(&person, "POST", "/deploy/preview", Some(Body::Json(json!({ "slug": "ticker", "unit": plan.unit, "requirements": requirements }))))?;
            let ask = json!({ "slug": "ticker", "unit": plan.unit, "acknowledge": preview["needs_approval"], "requirements": requirements, "document": plan.document });
            json_of(&person, "POST", "/deploy", Some(Body::Json(ask)))?;
            Ok(preview)
        };
        deploy_with(json!([])).unwrap();
        let refused = tool_as(&agent, "run", json!({ "slug": "ticker", "action": "reset", "input": { "to": 3 } })).unwrap_err();
        assert!(refused.contains("refused") && refused.contains("agents may not run"), "{refused}");
        let ran = tool_as(&person, "run", json!({ "slug": "ticker", "action": "reset", "input": { "to": 3 } })).unwrap();
        assert!(ran.starts_with("action reset → {\"set\":3,"), "a person runs it: {ran}");

        let preview = deploy_with(json!(["reset"])).unwrap();
        assert!(preview["needs_approval"].as_array().unwrap().contains(&json!("AgentRuns(\"reset\")")), "a person approves it: {preview}");
        let ran = tool_as(&agent, "run", json!({ "slug": "ticker", "action": "reset", "input": { "to": 7 } })).unwrap();
        assert!(ran.contains("\"set\":7") && ran.contains("This was run"), "{ran}");
        assert!(tool_as(&agent, "run", json!({ "slug": "ticker", "action": "reset" })).unwrap().contains("\"set\":0"), "nothing sent is {{}}");
        assert!(runs(&agent, "ticker", 5).unwrap().contains("ada (agent)"), "the run names who ran it");

        json_of(&person, "POST", "/deployments/ticker/grant", Some(Body::Json(json!({ "revoke": ["AgentRuns(\"reset\")"] })))).unwrap();
        assert!(tool_as(&agent, "run", json!({ "slug": "ticker", "action": "reset" })).unwrap_err().contains("agents may not run"));
        assert!(state.deployments.lock().unwrap()["ticker"].life.status == crate::lifecycle::Status::Running, "taken back, it runs on");
        assert!(deploy_with(json!(["nope"])).unwrap_err().contains("it has no action \"nope\""));
        assert_eq!(person.send("POST", "/deployments/ticker/stop", None).unwrap().0, 200);
    }

    fn tool_as(api: &dyn Api, name: &str, args: Value) -> Result<String, String> {
        let connect = || -> Result<Box<dyn Api + '_>, String> {
            struct By<'b>(&'b dyn Api);
            impl Api for By<'_> {
                fn server(&self) -> String {
                    self.0.server()
                }
                fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Vec<u8>), String> {
                    self.0.send(method, path, body)
                }
            }
            Ok(Box::new(By(api)))
        };
        call_tool(name, &args, &connect)
    }

    /// An agent's new code never inherits a secret a person let the old code read (redaction
    /// catches only the literal value, so the agent could read it out encoded), and an agent
    /// never starts a deployment a person stopped by deploying over it.
    #[test]
    fn an_agent_neither_inherits_a_secret_nor_starts_a_stopped_deployment() {
        let mut state = test_state();
        let accounts = std::sync::Arc::new(crate::accounts::Accounts::memory());
        let (ada, _) = accounts.setup(&accounts.setup_code().unwrap(), "ada", "correct horse battery").unwrap();
        state.access = crate::access::Access::new(std::sync::Arc::clone(&accounts), vec![], vec![]);
        let (agent, person) = (Signed { state: &state, token: &ada, agent: true }, Signed { state: &state, token: &ada, agent: false });
        let v1 = "flow peek\nport 8231\n\nroute GET /k (req) {\n  respond { k: sha256(secret.api_key) }, missing: 400\n}\n";
        let v2 = v1.replace("sha256", "base64");
        // A person deploys the version that hashes the key, and sets the key.
        let plan = agent::plan(v1).unwrap();
        for unit in &plan.units {
            agent.send("POST", "/unit", Some(Body::Bytes(unit.clone()))).unwrap();
        }
        let ask = |unit: &str, acknowledge: Value| Some(Body::Json(json!({ "slug": "peek", "unit": unit, "acknowledge": acknowledge, "document": plan.document })));
        let preview = json_of(&person, "POST", "/deploy/preview", Some(Body::Json(json!({ "slug": "peek", "unit": plan.unit })))).unwrap();
        json_of(&person, "POST", "/deploy", ask(&plan.unit, preview["needs_approval"].clone())).unwrap();
        json_of(&person, "POST", "/deployments/peek/secrets/api_key", Some(Body::Json(json!({ "value": "sk_live_not_for_agents" })))).unwrap();
        // The agent's version that would read it out encoded waits for a person.
        assert!(deploy(&agent, &v2, Some("peek")).unwrap().starts_with("Not deployed yet"));
        let leaky = agent::plan(&v2).unwrap();
        let preview = json_of(&agent, "POST", "/deploy/preview", Some(Body::Json(json!({ "slug": "peek", "unit": leaky.unit })))).unwrap();
        assert_eq!(preview["needs_approval"], json!(["SecretRead(\"api_key\")"]), "{preview}");
        let refused = json_of(&agent, "POST", "/deploy", ask(&leaky.unit, json!([]))).unwrap_err();
        assert!(refused.contains("needs a person's approval") && refused.contains("SecretRead(\"api_key\")"), "{refused}");
        let answered = json_of(&agent, "POST", "/deployments/peek/request", Some(Body::Json(json!({ "method": "GET", "url": "/k" })))).unwrap();
        assert!(!answered.to_string().contains("c2tfbGl2ZV"), "the key isn't read out as base64: {answered}");
        // The same code it may deploy again while it runs; once a person stops it, it stays stopped.
        json_of(&agent, "POST", "/deploy", ask(&plan.unit, json!([]))).unwrap();
        assert_eq!(person.send("POST", "/deployments/peek/stop", None).unwrap().0, 200);
        let restarted = json_of(&agent, "POST", "/deploy", ask(&plan.unit, json!([]))).unwrap_err();
        assert!(restarted.contains("is stopped") && restarted.contains("doesn't start"), "{restarted}");
        assert_eq!(json_of(&person, "GET", "/deployments", None).unwrap()["deployments"][0]["status"], "stopped");
    }

    /// A deploy that needs approval saves the flow for a person, but never over a flow someone
    /// else saved under that name.
    #[test]
    fn a_flow_someone_else_saved_is_not_replaced() {
        let text = holon_author::samples::sample("hello-api").unwrap().text;
        let plan = agent::plan(text).unwrap();
        let server = |saved_by: &'static str| {
            Given(vec![
                ("POST", "/unit", 200, json!({ "hash": plan.unit })),
                ("POST", "/deploy/preview", 200, json!({ "needs_approval": ["NetworkIngress"], "missing_secrets": [], "refused": null })),
                ("GET", "/auth/me", 200, json!({ "name": "local:me", "role": "admin" })),
                ("GET", "/flows/hello-api", 200, json!({ "name": "hello-api", "saved_by": saved_by })),
                ("POST", "/flows", 200, json!({ "name": "hello-api" })),
            ])
        };
        let refused = deploy(&server("ada"), text, None).unwrap_err();
        assert!(refused.contains("saved by ada") && refused.contains("another slug"), "{refused}");
        assert!(deploy(&server("local:me"), text, None).unwrap().starts_with("Not deployed yet"), "its own earlier proposal is replaced");
    }

    /// A saved flow reads back as the text the agent wrote; what is deployed reads in full,
    /// text and all, before it has ever run.
    #[test]
    fn what_is_saved_and_deployed_reads_back_as_text() {
        let state = test_state();
        assert!(tool(&state, "deployments", json!({})).unwrap().starts_with("Nothing is deployed"));
        assert!(tool(&state, "flows", json!({})).unwrap().starts_with("No flows are saved"));
        // Saved, not deployed: listed, and its text is the text as written, comments and all.
        let saved = tool(&state, "save", json!({ "text": SLUGS_TEXT })).unwrap();
        assert!(saved.starts_with("Saved \"slugs\"") && saved.contains("receive requests"), "{saved}");
        let listed = tool(&state, "flows", json!({})).unwrap();
        assert!(listed.contains("slugs: saved") && listed.contains("not deployed"), "{listed}");
        let text = tool(&state, "flows", json!({ "name": "slugs" })).unwrap();
        assert!(text.ends_with(SLUGS_TEXT.trim_end()), "{text}");
        assert!(tool(&state, "deployments", json!({})).unwrap().contains("Saved, never deployed"));
        assert!(tool(&state, "deployments", json!({ "slug": "slugs" })).unwrap().contains("isn't deployed. It is saved"));
        assert!(tool(&state, "deployments", json!({ "slug": "nobody" })).unwrap_err().contains("nothing named \"nobody\""));
        assert!(tool(&state, "flows", json!({ "name": "nobody" })).is_err());
        // A person deploys it; then it reads in full, though it has never run.
        let plan = agent::plan(SLUGS_TEXT).unwrap();
        for unit in &plan.units {
            route(&state, &req("POST", "/unit", unit.clone()));
        }
        let preview: Value = serde_json::from_slice(&body_bytes(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&json!({ "slug": "slugs", "unit": plan.unit })).unwrap())))).unwrap();
        let approve = json!({ "slug": "slugs", "unit": plan.unit, "acknowledge": preview["needs_approval"], "document": plan.document });
        assert_eq!(route(&state, &req("POST", "/deploy", serde_json::to_vec(&approve).unwrap())).status_code, 200);
        let all = tool(&state, "deployments", json!({})).unwrap();
        assert!(all.contains("slugs: running") && all.contains("0 runs, 0 failed"), "{all}");
        assert!(!all.contains("never deployed"), "{all}");
        let one = tool(&state, "deployments", json!({ "slug": "slugs" })).unwrap();
        assert!(one.contains("Its routes: POST /slugs"), "{one}");
        assert!(one.contains("It may:") && one.contains("receive requests"), "{one}");
        assert!(one.contains("Nothing waits for a person."), "{one}");
        assert!(one.contains("Its text, as deployed:") && one.contains("route POST /slugs"), "{one}");
        assert!(tool(&state, "flows", json!({})).unwrap().contains("deployed under that name"));
    }

    /// What a deployment waits on a person for, each said as what they do.
    #[test]
    fn a_deployment_says_what_it_waits_on_a_person_for() {
        let running = json!({ "slug": "s", "status": "running", "deployed_at_epoch_seconds": 100 });
        assert!(waits_for(&running, None).is_empty());
        assert!(waits_for(&running, Some(&json!({ "saved_at_epoch_seconds": 90 }))).is_empty(), "the version saved before is the one deployed");
        let newer = waits_for(&running, Some(&json!({ "saved_at_epoch_seconds": 200, "saved_by": "ada (agent)" })));
        assert!(newer[0].contains("saved since by ada (agent)") && newer[0].contains("flows page"), "{newer:?}");
        let stuck = json!({ "slug": "s", "status": "paused", "attention": "its grant changed", "listen_error": "port 8200 is taken", "deployed_at_epoch_seconds": 100 });
        let waits = waits_for(&stuck, None);
        assert_eq!(waits.len(), 3, "{waits:?}");
        assert!(waits[0].contains("its grant changed") && waits[1].contains("it is paused") && waits[2].contains("port 8200 is taken"), "{waits:?}");
    }

    #[test]
    fn the_protocol_initializes_lists_and_calls_tools_one_line_at_a_time() {
        let state = test_state();
        let connect = || -> Result<Box<dyn Api + '_>, String> { Ok(Box::new(InProcess(&state))) };
        let input = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"reference","arguments":{"topic":"concat"}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"check","arguments":{"text":"flow f\nport 8200\n\nroute GET /a (req) {\n  respond nope\n}\n"}}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope"}}"#,
            r#"{"jsonrpc":"2.0","id":6,"method":"resources/list"}"#,
            "not json",
            r#"{"jsonrpc":"2.0","id":7,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        ]
        .join("\n");
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, &connect).unwrap();
        let answers: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(answers.len(), 8, "a notification gets no answer");
        assert_eq!(answers[0]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(answers[0]["result"]["serverInfo"]["name"], "holon");
        let names: Vec<&str> = answers[1]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["reference", "check", "save", "deploy", "call", "run", "deployments", "flows", "runs", "explain"]);
        assert_eq!(answers[2]["result"]["isError"], false);
        assert!(answers[2]["result"]["content"][0]["text"].as_str().unwrap().starts_with("concat("));
        assert_eq!(answers[3]["result"]["isError"], true, "a mistake is a tool's failure, said: {}", answers[3]);
        assert!(answers[3]["result"]["content"][0]["text"].as_str().unwrap().contains("unknown name"));
        assert_eq!(answers[4]["error"]["code"], -32602);
        assert_eq!(answers[5]["error"]["code"], -32601);
        assert_eq!(answers[6]["error"]["code"], -32700);
        assert_eq!(answers[7]["result"]["protocolVersion"], VERSIONS[VERSIONS.len() - 1]);
    }
}
