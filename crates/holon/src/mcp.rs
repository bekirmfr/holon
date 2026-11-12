//! `holon mcp`: the tools an agent uses to write, deploy, call and understand flows, as a Model
//! Context Protocol server on stdin and stdout (docs/L4s/agent-surface-L4.md). It speaks the
//! language's text and answers in sentences; what it computes is holon-author's (`agent.rs`), and
//! what it asks a server is what the editor asks. The tools never approve: a deploy that needs a
//! person's approval is kept by the server as a proposal for them (proposals.rs), and says so.
//!
//! It signs in as an agent, a principal of its own, never as a person: on the server this user
//! runs, as theirs (`<user>-agent`, made by the run file's admin the first time: mcphosts.rs); on
//! a server elsewhere, with the agent's token setup kept. So the server holds it to what an agent
//! may do (can.rs), and what it did is under the agent's name.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crate::sync::Mutex;
use std::time::Duration;

use holon_author::agent;
use serde_json::{json, Value};

use crate::local;

/// The protocol versions this server speaks, newest last: it answers with the client's when it
/// knows it, else its newest.
const VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

const USAGE: &str = "usage: holon mcp [--server PORT|URL]
       holon mcp setup [HOST…|--all] [options]
       holon mcp hosts [add NAME --config PATH --like HOST | remove NAME]

  An MCP server on stdin and stdout, for an agent's host (Claude Code, an IDE) to start: tools
  to look up the language, check a flow, deploy it, call it, list its runs and explain one, read
  a deployment's history, and see what a restore would change (only ever a dry run). Its tools
  never approve what a flow may do: a deploy that needs approval is a proposal, which waits in
  the editor's Approvals for a person. It signs in as an agent, so the server holds it to
  proposing too.

  --server PORT|URL        which holon server to ask (see below)
  setup                    add it to a host's settings (holon mcp setup --help says more)
  hosts                    the hosts setup knows, and adding your own (holon mcp hosts --help)

examples:
  holon mcp setup claude-code
  holon mcp setup --all --print
  holon mcp hosts

  The holon serve you run on this machine, as your agent (<you>-agent, owned by you, made the
  first time); with several running, --server <port> picks one. A server elsewhere: --server
  <URL> (or HOLON_SERVER), with the agent's token holon mcp setup --server kept (or HOLON_TOKEN).";

const INSTRUCTIONS: &str = "Holon runs small services (flows) written in its own language, and \
shows what each may do before it runs. Call `reference` first for the language and its leaves, \
`check` a text before deploying it, then `deploy`, `call` and `explain`; `deployments` and `flows` \
show what is already there, with its text, and `save` keeps a flow without deploying it; \
`whoami` says what you may do, and where. Before deploying a new version of a deployed flow, `replay` runs it offline on the flow's recent runs and says which it answers differently: say so in the deploy's why. When a flow begins to answer differently, `history` shows what changed before it, and `restore` what going back would change; a person restores it. A first \
deploy, and any version that needs a new permission, waits for a person to approve it in the \
editor: tell the user what it asks for and where, and deploy again once they have. A proposal \
is decided later, and a decline comes with a reason: with a shell, run `holon wait <slug>` in the \
background (it ends when a person decides, exit 0 approved, 1 declined with the reason); without \
one, `deployments` with the slug and `wait` holds until they do. Read the outcome before you \
deploy again. A run that \
waits at an approval (`await approval(…)`) goes on when people decide it, in the editor's \
Approvals or with `holon decide`; no tool decides one: `runs` shows which run waits, and who has \
approved it so far, for you to tell the user.";

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

/// A server over HTTP, signed in with `token`.
pub(crate) struct Http {
    server: String,
    token: Option<String>,
}

impl Http {
    pub(crate) fn new(server: &str, token: Option<&str>) -> Http {
        Http { server: server.to_string(), token: token.map(str::to_string) }
    }
}

impl Api for Http {
    fn server(&self) -> String {
        self.server.clone()
    }
    fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Vec<u8>), String> {
        let url = format!("{}{path}", self.server);
        let mut request = ureq::request(method, &url).timeout(Duration::from_secs(60));
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
pub(crate) fn json_of(api: &dyn Api, method: &str, path: &str, body: Option<Body>) -> Result<Value, String> {
    let (status, bytes) = api.send(method, path, body)?;
    let value: Value = crate::wire::read_json(&bytes).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok(value);
    }
    let why = value.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| String::from_utf8_lossy(&bytes).trim().to_string());
    Err(match status {
        401 => format!("{} wants a sign-in ({why}): holon mcp setup --server {} keeps an agent's token for it", api.server(), api.server()),
        403 => format!("{} refused ({why})", api.server()),
        _ => format!("{} answered {status}: {why}", api.server()),
    })
}

/// A path segment, escaped.
pub(crate) fn seg(s: &str) -> String {
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
            "description": "Deploy a flow to the Holon server. It never approves anything: when the flow may do something not yet approved (a first deploy always is), it is proposed for a person, with your why and note, and the answer says what they must approve and where. They approve it or decline it with a reason, which `deployments` with its slug says. Once a version is deployed, a new one that needs nothing more deploys from here at once.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": text,
                    "slug": slug,
                    "why": { "type": "string", "description": "what this version changes and why, in one line, for the person who reviews it" },
                    "note": { "type": "string", "description": "more for them, if any: who asked for it, what you tried" }
                },
                "required": ["text"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
        },
        {
            "name": "replay",
            "title": "Replay a new version on recent runs",
            "description": "Run a new version of a deployed flow, offline, on the deployment's recent runs, before deploying it: how many it answers the same, and for each other run how it differs (the status, the answer, the effects it performs, where it waits) or why it can't be told. Nothing is deployed, sent or written: each request the version sends is answered as that run was answered, so a run where it asks what the run never did, or computes with a secret, can't be told.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": text,
                    "slug": { "type": "string", "description": "the deployment to replay on: the flow's name unless deployed under another" },
                    "runs": { "type": "integer", "minimum": 1, "maximum": 200, "description": "how many of its newest runs (default 20)" }
                },
                "required": ["text"]
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
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
            "name": "whoami",
            "title": "Who you are signed in as",
            "description": "Who you are signed in as on the server (your name, your kind, the person who owns you) and what you may do: each role bound to you or to a group you are in, where it holds (this host, one flow, one approval point), until when, and the actions it allows. What none of them allows, a person does.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "deployments",
            "title": "List what is deployed",
            "description": "What is deployed, one line each (state, version, port, runs, who deployed it), what each waits on a person for, and the flows saved but never deployed. With a slug, that one in full: its routes, what it may do, what its grant doesn't cover, the secrets it reads and which are set, what it waits for, and its text as deployed, even when it has never run.",
            "inputSchema": { "type": "object", "properties": {
                "slug": { "type": "string", "description": "one deployment's name; leave out for all" },
                "wait": { "type": "integer", "minimum": 1, "maximum": 600, "description": "with a slug whose proposal waits for a person: seconds to hold the answer for their decision; it answers as soon as they decide, and says it is still waiting if they haven't. Up to 45, or up to 600 when the request asks for progress (it is told every second, and a cancel stops it)" }
            } },
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
            "description": "A deployed flow's latest runs, one line each: what started it, its answer, how long it took, and why it failed. Then the runs waiting now: at an approval, with who has approved it so far (a person decides it; no tool does), or at an event.",
            "inputSchema": { "type": "object", "properties": { "slug": slug, "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "how many, newest last (default 10)" } }, "required": ["slug"] },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "logs",
            "title": "A flow's log lines",
            "description": "What a deployed flow wrote with console.log, info, debug, warn and error, oldest first, one line each: when, its level, the run that wrote it, and the text. They are kept beside the runs, not in them. Only where a person let agents read them (the deploy settings' \"agents may read its logs\", which asks for AgentReadsLogs): otherwise this is refused, and the person is the one to ask.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": slug,
                    "level": { "type": "string", "enum": ["warn", "error"], "description": "only this level and louder (default every line)" },
                    "run": { "type": "integer", "minimum": 0, "description": "a run's number: the lines up to the end of that run" },
                    "own": { "type": "boolean", "description": "with run, only the lines that run wrote" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500, "description": "the newest this many (default 50)" }
                },
                "required": ["slug"]
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "state",
            "title": "A deployment's state",
            "description": "What a deployed flow keeps now: each state value, as the language writes it, and its collections by name. With a collection, its items (key and value), 50 at a time from a key on; with a key too, that one item.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": slug,
                    "collection": { "type": "string", "description": "a collection's name: its items instead" },
                    "key": { "type": "string", "description": "with a collection, one item's key" },
                    "prefix": { "type": "string", "description": "with a collection, only the keys starting with this" },
                    "after": { "type": "string", "description": "with a collection, the keys after this one (the next page)" }
                },
                "required": ["slug"]
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "explain",
            "title": "Explain a run",
            "description": "One recorded run as the program: the flow's text with the value each statement had, where a missing value came from, what it answered, the effects it performed and the calls it made. The latest run unless one is named.",
            "inputSchema": { "type": "object", "properties": { "slug": slug, "run": { "type": "integer", "description": "the run's number (from call or runs); the latest when left out" } }, "required": ["slug"] },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "history",
            "title": "A deployment's history",
            "description": "A deployment's timeline, oldest first: its runs, each with its number and the version it ran on, and what was done to it between them, by whom: deploys, restores, grant changes, secrets set or rolled back (never their values), folders and databases bound, pausing and stopping. Where to look when a flow began to answer differently: what changed just before. With a cell, that one value's history instead: the runs that changed it (state.NAME, a collection, or one item of it), each with its value before and after, and a restore or deploy between runs that moved it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": slug,
                    "last": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "the newest runs to show, with the events among them (default 20)" },
                    "events": { "type": "boolean", "description": "every event, kept for good, without the runs" },
                    "cell": { "type": "string", "description": "a cell's name (moves, for state.moves) or a collection's (balances): its history instead" },
                    "item": { "type": "string", "description": "with a collection as cell, one item's key: balances[\"a\"] is cell balances, item a" }
                },
                "required": ["slug"]
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "restore",
            "title": "What a restore would change, or ask a person for it",
            "description": "A dry run of going back to an earlier version or run: nothing changes. The code it would deploy again (its text beside the text deployed now, and the permissions it would add or drop), the state keys and collection items it would put back (now → then), the runs since that point with what they did outside their state, which a restore doesn't undo, and the files they wrote, an edit made outside a run named. It never restores: a person does, in the editor or with `holon restore`. With `ask`, it saves the restore for a person to approve, on the deployment's timeline, where the editor's history offers it; tell the user what it would change and where to approve it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": slug,
                    "version": { "type": "integer", "description": "a version's number (history shows which version each run ran on)" },
                    "run": { "type": "integer", "description": "a run's number: the state it left, and the code it ran on" },
                    "code": { "type": "boolean", "description": "deploy that point's code again" },
                    "state": { "type": "boolean", "description": "put back the state that point left" },
                    "ask": { "type": "boolean", "description": "save it for a person to approve, once the user wants it restored: it restores nothing itself" }
                },
                "required": ["slug"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false }
        }
    ])
}

/// How the tools get a server, when one is needed. `Sync`: a call runs on a thread of its own.
pub(crate) type Connect<'a> = dyn Fn() -> Result<Box<dyn Api + 'a>, String> + Sync + 'a;

/// What a call that takes a while can be told, and can tell: that whoever asked has given up
/// (`notifications/cancelled`), and how far it has got (`notifications/progress`, when the
/// request carried a progress token, which hosts take as a sign the call is alive).
#[derive(Default)]
pub(crate) struct Ctx<'a> {
    cancelled: Option<&'a AtomicBool>,
    progress: Option<&'a Progress<'a>>,
}

/// What a call says of how far it has got: done, of how many, and in a few words.
type Progress<'p> = dyn Fn(u64, u64, &str) + Sync + 'p;

impl Ctx<'_> {
    fn is_cancelled(&self) -> bool {
        self.cancelled.is_some_and(|c| c.load(Ordering::Relaxed))
    }

    fn report(&self, done: u64, of: u64, note: &str) {
        if let Some(progress) = self.progress {
            progress(done, of, note);
        }
    }
}

fn arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name).and_then(Value::as_str).ok_or_else(|| format!("`{name}` is missing"))
}

/// Run the tool `name`: its answer, or why it failed (both sentences).
pub(crate) fn call_tool(name: &str, args: &Value, connect: &Connect, ctx: &Ctx) -> Result<String, String> {
    match name {
        "reference" => agent::reference(args.get("topic").and_then(Value::as_str)),
        "check" => {
            let r = holon_author::commands::check("text", arg(args, "text")?);
            let out = r.out.join("\n");
            if r.code == 0 { Ok(out) } else { Err([out, r.err.join("\n")].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("\n")) }
        }
        "save" => save(&*connect()?, arg(args, "text")?, args.get("name").and_then(Value::as_str)),
        "deploy" => {
            let said = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
            deploy(&*connect()?, arg(args, "text")?, said("slug"), Asked { why: said("why"), note: said("note") })
        }
        "replay" => {
            let on = args.get("slug").and_then(Value::as_str);
            crate::replaycmd::replay(&*connect()?, arg(args, "text")?, on, args.get("runs").and_then(Value::as_u64).map(|n| n.clamp(1, 200))).map(|said| said.text)
        }
        "call" => call(&*connect()?, args),
        "run" => run_action(&*connect()?, arg(args, "slug")?, arg(args, "action")?, args.get("input").cloned().unwrap_or_else(|| json!({}))),
        "deployments" => {
            let api = connect()?;
            let slug = args.get("slug").and_then(Value::as_str).filter(|s| !s.trim().is_empty());
            let waited = match (slug, args.get("wait").and_then(Value::as_u64)) {
                // Past what a host gives a tool that says nothing, only for one that is told how it goes.
                (Some(slug), Some(seconds)) if seconds > 0 => {
                    let most = if ctx.progress.is_some() { MAX_WAIT_REPORTED } else { MAX_WAIT };
                    wait_for_decision(&*api, slug, seconds.min(most), Duration::from_secs(1), ctx)
                }
                _ => None,
            };
            deployments(&*api, slug).map(|said| match waited {
                Some(note) => format!("{note}\n\n{said}"),
                None => said,
            })
        }
        "flows" => flows(&*connect()?, args.get("name").and_then(Value::as_str).filter(|s| !s.trim().is_empty())),
        "whoami" => whoami(&*connect()?),
        "runs" => runs(&*connect()?, arg(args, "slug")?, args.get("limit").and_then(Value::as_u64).unwrap_or(10).clamp(1, 200)),
        "explain" => explain(&*connect()?, arg(args, "slug")?, args.get("run").and_then(Value::as_u64)),
        "logs" => logs(&*connect()?, arg(args, "slug")?, args),
        "state" => state(&*connect()?, arg(args, "slug")?, args),
        "history" => {
            let last = args.get("last").and_then(Value::as_u64).unwrap_or(20).clamp(1, 1000);
            match args.get("cell").and_then(Value::as_str).filter(|c| !c.trim().is_empty()) {
                Some(cell) => cell_history(&*connect()?, arg(args, "slug")?, cell, args.get("item").and_then(Value::as_str), last),
                None => history(&*connect()?, arg(args, "slug")?, last, args.get("events").and_then(Value::as_bool) == Some(true)),
            }
        }
        "restore" => restore(&*connect()?, args),
        other => Err(format!("there is no tool \"{other}\"")),
    }
}

/// Who the tools are signed in as, and what each binding of theirs allows, where: what the
/// server answers on `/auth/me`, which an agent reads though it may not read the roles.
fn whoami(api: &dyn Api) -> Result<String, String> {
    let me = json_of(api, "GET", "/auth/me", None)?;
    let name = me["name"].as_str().unwrap_or("-");
    let kind = me["kind"].as_str().unwrap_or("principal");
    let a = if kind.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
    let owner = me["owner"].as_str().map(|o| format!(", owned by {o}")).unwrap_or_default();
    let mut lines = vec![format!("You are {name}, {a} {kind}{owner}, on {}.", api.server())];
    let bindings = me["bindings"].as_array().cloned().unwrap_or_default();
    match me["role"].as_str() {
        _ if !bindings.is_empty() => {
            lines.push("You hold:".into());
            for b in &bindings {
                let via = b["group"].as_str().map(|g| format!(" (through the group {g})")).unwrap_or_default();
                let until = match b["expires_at"].as_u64() {
                    _ if b["expired"] == true => ", expired: it holds nothing now".into(),
                    Some(at) => format!(", until {}", crate::cmdline::when(Some(at))),
                    None => String::new(),
                };
                let allows: Vec<&str> = b["allows"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                lines.push(format!("  - {} {}{via}{until}: {}", b["role"].as_str().unwrap_or_default(), crate::accesscmd::place(&b["scope"]), allows.join(", ")));
            }
        }
        Some(role) => lines.push(format!("You hold {role} on this host.")),
        None => lines.push("Nothing is bound to you: you may do nothing here until a person binds you a role (holon bind).".into()),
    }
    if kind == "agent" {
        lines.push("Whatever you hold, a person approves a deploy that needs it, starts, stops, deletes and restores a flow, writes its secrets, decides its approvals, and reads and changes people, roles and resources.".into());
    }
    Ok(lines.join("\n"))
}

/// What an agent says of a version it proposes, for the person who reviews it.
#[derive(Default, Clone, Copy)]
struct Asked<'a> {
    why: Option<&'a str>,
    note: Option<&'a str>,
}

fn deploy(api: &dyn Api, text: &str, slug: Option<&str>, asked: Asked) -> Result<String, String> {
    let plan = agent::plan(text)?;
    let slug = slug.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&plan.name).to_string();
    for unit in &plan.units {
        json_of(api, "POST", "/unit", Some(Body::Bytes(unit.clone())))?;
    }
    let mut body = json!({ "slug": slug, "unit": plan.unit });
    if let Some(r) = &plan.requirements {
        body["requirements"] = r.clone();
    }
    if let Some(b) = &plan.browsers {
        // The plan keeps the origins on one line (`https://a.example *`); the deploy request
        // takes them one by one, as the editor sends them. The preview gets them too: answering
        // pages from a new site is a person's to approve.
        body["browsers"] = json!(b.split_whitespace().collect::<Vec<_>>());
    }
    let preview = json_of(api, "POST", "/deploy/preview", Some(Body::Json(body.clone())))?;
    if let Some(why) = preview.get("refused").and_then(Value::as_str) {
        return Err(format!("this server won't run it: {why}"));
    }
    let list = |k: &str| -> Vec<String> { preview.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default() };
    let (needs, missing) = (list("needs_approval"), list("missing_secrets"));
    let described = |items: &[String]| items.iter().map(|i| format!("  - {}", agent::describe_sending(i, &plan.sent))).collect::<Vec<_>>().join("\n");
    let secrets_note = if missing.is_empty() {
        String::new()
    } else {
        format!("\nSecrets not set yet (an admin sets them in the editor, in the flow's Settings): {}. Until then, a run that reads one gets nothing.", missing.join(", "))
    };
    // Its text goes with it, comments and all, so the person who opens it reads what was written.
    body["document"] = saved_document(&plan, text);
    if let Some(w) = asked.why {
        body["why"] = json!(w);
    }
    if let Some(n) = asked.note {
        body["note"] = json!(n);
    }
    let deployed = json_of(api, "POST", "/deploy", Some(Body::Json(body)))?;
    // What needs a person's approval, the server keeps as a proposal for them.
    if let Some(p) = deployed.get("proposal") {
        let touches = crate::touches::text(&crate::touches::lines(&preview["touches"], &preview["settings"]));
        let version = p.get("version").and_then(Value::as_u64).unwrap_or(1);
        let why = if asked.why.is_some() { "with your why" } else { "with no why: say what it changes and why, with `why`, next time" };
        return Ok(format!(
            "Not deployed yet: proposed as version {version} of \"{slug}\" (proposal {}), {why}. A person has to approve what it may do. It asks to:\n{}\n\nWhat it touches (+ new, - no longer, ~ changed, ! not ready on {}):\n{}\n\nIt waits in the editor's Approvals on {} (#approvals.{slug}.v{version}), where they approve it, which deploys it, or decline it with a reason. Tell them, and hear what they decide without polling: run `holon wait {slug} --server {}` in the background (it ends when they decide, exit 0 if approved, 1 if declined, with the reason), or `deployments` with \"{slug}\" and `wait` where your host has no shell. Once it is deployed, a version that needs nothing more deploys from here at once.{secrets_note}",
            p.get("id").and_then(Value::as_u64).unwrap_or(0),
            described(&needs),
            api.server(),
            touches.join("\n"),
            api.server(),
            api.server()
        ));
    }
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
    } else if let Some(t) = plan.document["trace"].as_object().filter(|t| !t.is_empty()) {
        out.push(format!("What starts its runs: {}.", t.keys().cloned().collect::<Vec<_>>().join(", ")));
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
pub(crate) fn saved_document(plan: &agent::Plan, text: &str) -> Value {
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
    let may = if plan.may.is_empty() { "It only computes.".to_string() } else { format!("When deployed, it may:\n{}", plan.may.iter().map(|i| format!("  - {}", agent::describe_sending(i, &plan.sent))).collect::<Vec<_>>().join("\n")) };
    Ok(format!(
        "Saved \"{name}\" on {}, not deployed: it is on the editor's flows page, where a person can open it, and `deploy` deploys it (a first deploy, or one that needs a new permission, waits for them).\n{may}",
        api.server()
    ))
}

/// When, as a person reads it.
pub(crate) fn when(v: &Value) -> String {
    v.as_u64().map(agent::utc).unwrap_or_else(|| "?".into())
}

/// The things a deployment waits on a person for, each said as what they do.
pub(crate) fn waits_for(entry: &Value, saved: Option<&Value>) -> Vec<String> {
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
    } else if let Some(t) = d.get("triggers").and_then(Value::as_array).filter(|t| !t.is_empty()) {
        line.push_str(&format!(", {}", t.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")));
    }
    let runs = n("trigger_count");
    line.push_str(&format!("; {runs} run{}, {} failed", if runs == 1 { "" } else { "s" }, n("error_count")));
    if n("waiting") > 0 {
        line.push_str(&format!(", {} waiting (`runs` says for what)", n("waiting")));
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
    let proposed = proposals_of(api, slug);
    let Some(d) = all.iter().find(|d| d.get("slug").and_then(Value::as_str) == Some(slug)) else {
        return match saved(slug) {
            Some(f) => Ok(format!(
                "\"{slug}\" isn't deployed. It is saved{} at {}, for a person to deploy from the flows page; `flows` with its name shows its text.{}",
                f.get("saved_by").and_then(Value::as_str).map(|b| format!(" by {b}")).unwrap_or_default(),
                when(&f["saved_at_epoch_seconds"]),
                proposed.map(|p| format!("\n{p}")).unwrap_or_default()
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
        let named: Vec<String> = reads.iter().map(|r| if secrets.iter().any(|s| s.as_str() == Some(r)) { format!("{r} (set)") } else { format!("{r} (not set: an admin sets it in the flow's Settings)") }).collect();
        out.push(format!("The secrets it reads: {}", named.join(", ")));
    }
    let waits = waits_for(d, saved(slug));
    out.push(if waits.is_empty() { "Nothing waits for a person.".into() } else { format!("It waits for a person:\n{}", waits.iter().map(|w| format!("  - {w}")).collect::<Vec<_>>().join("\n")) });
    out.extend(proposed);
    match text {
        Ok(t) => out.push(format!("\nIts text, as deployed:\n\n{}", t.trim_end())),
        Err(e) => out.push(format!("\nIts text can't be shown: {e}")),
    }
    Ok(out.join("\n"))
}

/// How many of a flow's proposals `deployments` says, the waiting one first, then the newest.
const PROPOSALS_SAID: usize = 3;

/// The longest a call waits for a person, in seconds: under what a host gives a tool before it
/// gives up on it. An agent that wants longer asks again.
const MAX_WAIT: u64 = 45;

/// The longest when the request asked for progress: it is told every second that the call is
/// alive, and can be cancelled.
const MAX_WAIT_REPORTED: u64 = 600;

/// Whether a person has a proposal of `slug`'s still to decide.
fn waits_on_a_person(api: &dyn Api, slug: &str) -> bool {
    json_of(api, "GET", "/proposals", None)
        .map(|all| list_of(all, Some("proposals")).iter().any(|p| p.get("slug").and_then(Value::as_str) == Some(slug) && p.get("state").and_then(Value::as_str) == Some("waiting")))
        .unwrap_or(false)
}

/// What an agent does with a proposal that waits for a person: `deployments` with `wait` holds
/// the answer until it is decided, up to `seconds`, looking every `every`. Holon has no way to
/// push the decision to an agent that a host would carry to its model, so the agent asks and
/// the call is the wait (`holon wait` is the other way, for an agent with a shell). Each look
/// says how long it has waited, when the request asked, and a cancelled call stops within a
/// fraction of a second. None when nothing waits (what to say is the answer itself).
fn wait_for_decision(api: &dyn Api, slug: &str, seconds: u64, every: Duration, ctx: &Ctx) -> Option<String> {
    if !waits_on_a_person(api, slug) {
        return None;
    }
    let started = std::time::Instant::now();
    let until = started + Duration::from_secs(seconds);
    while std::time::Instant::now() < until {
        // Asleep in small parts, so a cancel is heard at once.
        let looked = std::time::Instant::now();
        while looked.elapsed() < every {
            if ctx.is_cancelled() {
                return Some("Stopped: the call was cancelled.".into());
            }
            std::thread::sleep(every.min(Duration::from_millis(50)));
        }
        if !waits_on_a_person(api, slug) {
            return Some("A person decided while you waited:".into());
        }
        ctx.report(started.elapsed().as_secs(), seconds, "waiting for a person to decide");
    }
    Some(format!("Still waiting for a person after {seconds} seconds: call `deployments` with this slug and `wait` again to keep watching."))
}

/// A proposal as its author reads what became of it: waiting, with who proposed it and why, or
/// declined with its reason, approved, or replaced. `holon wait` says it too.
pub(crate) fn proposal_line(p: &Value) -> String {
    let s = |k: &str| p.get(k).and_then(Value::as_str);
    let at = |k: &str| when(&json!(p.get(k).and_then(Value::as_u64).unwrap_or(0) / 1000));
    let id = p.get("id").and_then(Value::as_u64).unwrap_or(0);
    let decider = s("decided_by").unwrap_or("a person");
    match s("state").unwrap_or("?") {
        "waiting" => format!(
            "proposal {id}, version {}: waits for a person (proposed by {} at {}){}",
            p.get("version").and_then(Value::as_u64).unwrap_or(0),
            s("by").unwrap_or("someone"),
            at("at_ms"),
            s("why").filter(|w| !w.is_empty()).map(|w| format!(": {w}")).unwrap_or_default()
        ),
        "declined" => format!("proposal {id} was declined by {decider} at {}: {}", at("decided_at_ms"), s("reason").unwrap_or("no reason given")),
        "approved" => format!("proposal {id} was approved by {decider} at {}, and deployed", at("decided_at_ms")),
        _ => format!("proposal {id} was replaced by a newer one"),
    }
}

/// A flow's proposals, as its author reads what became of them: the one waiting, and the newest
/// decided, a declined one with its reason. None when it has none (or the server keeps none).
fn proposals_of(api: &dyn Api, slug: &str) -> Option<String> {
    let all = list_of(json_of(api, "GET", "/proposals", None).ok()?, Some("proposals"));
    let lines: Vec<String> = all
        .iter()
        .filter(|p| p.get("slug").and_then(Value::as_str) == Some(slug))
        .take(PROPOSALS_SAID)
        .map(|p| format!("  - {}", proposal_line(p)))
        .collect();
    (!lines.is_empty()).then(|| format!("Its proposals, the newest first:\n{}", lines.join("\n")))
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
    let before = json_of(api, "GET", &format!("{log}?limit=0"), None)?;
    let next = before.get("next").and_then(Value::as_u64).unwrap_or(0);
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
    let page = json_of(api, "GET", &format!("{log}?{after}limit=1000"), None)?;
    let run = page
        .get("entries")
        .and_then(Value::as_array)
        .and_then(|e| e.iter().find(|e| e.get("method").and_then(Value::as_str) == Some(method.as_str()) && e.get("url").and_then(Value::as_str) == Some(path.as_str())).cloned());
    out.push(String::new());
    match run.as_ref().and_then(|r| r.get("seq")).and_then(Value::as_u64) {
        Some(seq) if run.as_ref().and_then(|r| r.get("run_id")).and_then(Value::as_str).is_some_and(|id| !id.is_empty()) => {
            out.push(format!("This was run {seq}: `explain` with run {seq} shows what each statement did."));
        }
        Some(_) => out.push("No route matched it, so no run was made.".into()),
        None => out.push(match counted(&before, &page) {
            Some(label) => format!("Runs of {label} aren't recorded (its line says unrecorded): this one was counted, not logged, so there is no run to explain."),
            None => "Its run isn't in the log.".into(),
        }),
    }
    Ok(out.join("\n"))
}

/// The unrecorded handler whose count went up between two pages of a log: the handler of a run
/// that was counted, not logged.
fn counted(before: &Value, after: &Value) -> Option<String> {
    let runs = |page: &Value, label: &str| page.get("tallies").and_then(|t| t.get(label)).and_then(|t| t.get("runs")).and_then(Value::as_u64).unwrap_or(0);
    after.get("tallies")?.as_object()?.keys().find(|label| runs(after, label) > runs(before, label)).cloned()
}

/// Run `slug`'s action `name` with `input`: what it answered, and its run.
fn run_action(api: &dyn Api, slug: &str, name: &str, input: Value) -> Result<String, String> {
    let ran = json_of(api, "POST", &format!("/trigger/{}/{}", seg(slug), seg(name)), Some(Body::Json(input)))?;
    let Some(seq) = ran.get("seq").and_then(Value::as_u64) else {
        let answer = ran.get("answer").cloned().unwrap_or(Value::Null);
        return Ok(format!("action {name} → {answer}\n\nRuns of action {name} aren't recorded (its line says unrecorded): this one was counted, not logged, so there is no run to explain."));
    };
    if ran.get("waiting").and_then(Value::as_bool) == Some(true) {
        return Ok(format!("action {name} is waiting: run {seq}, which `explain` shows; `runs` says what it waits for, an approval or an event."));
    }
    let answer = ran.get("answer").cloned().unwrap_or(Value::Null);
    Ok(format!("action {name} → {answer}\n\nThis was run {seq}: `explain` with run {seq} shows what each statement did."))
}

fn runs(api: &dyn Api, slug: &str, limit: u64) -> Result<String, String> {
    let log = json_of(api, "GET", &format!("/deployments/{}/log?limit={limit}", seg(slug)), None)?;
    let entries = log.get("entries").and_then(Value::as_array).cloned().unwrap_or_default();
    let waiting = list_of(json_of(api, "GET", &format!("/deployments/{}/runs", seg(slug)), None)?, Some("runs"));
    if entries.is_empty() && waiting.is_empty() {
        return Ok(format!("\"{slug}\" has no runs yet."));
    }
    let mut out: Vec<String> = entries.iter().map(agent::run_line).collect();
    if let Some(breaks) = log.get("breaks").and_then(Value::as_array).filter(|b| !b.is_empty()) {
        out.push(format!("The log was changed outside holon before runs {}: their record can't be trusted.", breaks.iter().map(Value::to_string).collect::<Vec<_>>().join(", ")));
    }
    let points = crate::decidecmd::waiting_lines(&waiting);
    if !points.is_empty() {
        out.push(String::new());
        out.push("Waiting now:".into());
        out.extend(points);
        if waiting.iter().flat_map(|r| r["waiting"].as_array().cloned().unwrap_or_default()).any(|w| w["people"].as_u64().unwrap_or(0) > 0) {
            out.push(format!("An approval is a person's to decide, and no tool decides it: tell the user which run waits, and that they decide it in the editor (Approvals, Runs waiting) or with `holon decide {slug} <run> <name> --approve` (or --decline --reason …)."));
        }
    }
    Ok(out.join("\n"))
}

/// `slug`'s log lines, as `holon logs` prints them: oldest first, at a level or louder, up to a run's end
/// or only its own. The server refuses an agent where no grant lets it (`AgentReadsLogs`).
fn logs(api: &dyn Api, slug: &str, args: &Value) -> Result<String, String> {
    let mut asked = format!("limit={}", args.get("limit").and_then(Value::as_u64).unwrap_or(50).clamp(1, 500));
    if let Some(level) = args.get("level").and_then(Value::as_str).filter(|l| matches!(*l, "warn" | "error")) {
        asked.push_str(&format!("&level={level}"));
    }
    if let Some(run) = args.get("run").and_then(Value::as_u64) {
        asked.push_str(&format!("&{}={run}", if args.get("own").and_then(Value::as_bool) == Some(true) { "run" } else { "until" }));
    }
    let answer = json_of(api, "GET", &format!("/deployments/{}/logs?{asked}", seg(slug)), None)?;
    let lines = answer.get("lines").and_then(Value::as_array).cloned().unwrap_or_default();
    if lines.is_empty() {
        return Ok(format!("\"{slug}\" has written no lines (console.log in its flow writes them)."));
    }
    Ok(lines.iter().map(crate::logscmd::shown).collect::<Vec<_>>().join("\n"))
}

/// `slug`'s state, as `holon state` prints it; or one of its collections' items.
fn state(api: &dyn Api, slug: &str, args: &Value) -> Result<String, String> {
    let text = |name: &str| args.get(name).and_then(Value::as_str).filter(|s| !s.is_empty());
    let path = format!("/deployments/{}", seg(slug));
    let Some(name) = text("collection") else {
        let v = json_of(api, "GET", &format!("{path}/state"), None)?;
        return Ok(crate::opscmd::state_lines(slug, &v["state"]).join("\n").replace(&format!("holon state {slug} --collection "), "`state` with collection "));
    };
    let mut query = format!("{path}/collections/{}?limit=50", seg(name));
    for (param, given) in [("key", text("key")), ("prefix", text("prefix")), ("after", text("after"))] {
        if let Some(g) = given {
            query.push_str(&format!("&{param}={}", seg(g)));
        }
    }
    let v = json_of(api, "GET", &query, None)?;
    let items = v["items"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        return Ok(format!("{name} has no such item."));
    }
    let mut lines: Vec<String> = items.iter().map(|i| format!("{:?} → {}", i[0].as_str().unwrap_or_default(), agent::show(&i[1]))).collect();
    if let Some(next) = v["next"].as_str() {
        lines.push(format!("More: `state` with after {next:?}."));
    }
    Ok(lines.join("\n"))
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

/// `slug`'s timeline, as `holon history` prints it: its newest `last` runs and the events among
/// them, or every event.
fn history(api: &dyn Api, slug: &str, last: u64, events_only: bool) -> Result<String, String> {
    let path = format!("/deployments/{}", seg(slug));
    let events = list_of(json_of(api, "GET", &format!("{path}/events"), None)?, Some("events"));
    let page = if events_only { Value::Null } else { json_of(api, "GET", &format!("{path}/log?limit={last}"), None)? };
    let counted = crate::history::unrecorded(&page);
    let runs = if events_only { Vec::new() } else { list_of(page, Some("entries")) };
    let versions = crate::history::Versions::of(&json_of(api, "GET", &format!("{path}/versions"), None)?);
    let mut lines = crate::history::timeline(&runs, &events, &versions, events_only);
    lines.extend(counted);
    if lines.is_empty() {
        return Ok(format!("\"{slug}\" has nothing in its history yet."));
    }
    let shown = if events_only { "every event".to_string() } else { format!("its newest {} run(s), with the events among them", runs.len()) };
    Ok(format!(
        "{slug}'s history, oldest first ({shown}): `#N` is run N, `vN` the version it ran on, and an event comes before the run after it.\n\n{}\n\n`explain` with a run's number shows what it did; `restore` with a version or a run says what going back to it would change.",
        lines.join("\n")
    ))
}

/// One cell's history: the runs that changed it, oldest first, with its value before and after.
fn cell_history(api: &dyn Api, slug: &str, cell: &str, item: Option<&str>, last: u64) -> Result<String, String> {
    let path = format!("/deployments/{}", seg(slug));
    let item = item.map(|i| format!("&item={}", seg(i))).unwrap_or_default();
    let answer = json_of(api, "GET", &format!("{path}/changes?name={}{item}&limit={}", seg(cell), last.min(200)), None)?;
    let versions = crate::history::Versions::of(&json_of(api, "GET", &format!("{path}/versions"), None)?);
    let lines = crate::history::change_lines(&answer, &versions);
    Ok(format!(
        "{}\n\n`explain` with a run's number shows how it came to that value; `restore` with the run before says what putting it back would change.",
        lines.join("\n")
    ))
}

/// What going back to a version or a run would change, as the server's dry run answers it. An
/// agent restores nothing (deploy.rs refuses it more than a dry run): a person does.
fn restore(api: &dyn Api, args: &Value) -> Result<String, String> {
    let slug = arg(args, "slug")?;
    let flag = |k: &str| args.get(k).and_then(Value::as_bool) == Some(true);
    let (code, state) = (flag("code"), flag("state"));
    let (point, at, to) = match (args.get("version").and_then(Value::as_u64), args.get("run").and_then(Value::as_u64)) {
        (Some(n), None) => (format!("version {n}"), json!({ "version": n }), format!("--to {n}")),
        (None, Some(seq)) => (format!("run {seq}"), json!({ "run": seq }), format!("--to-run {seq}")),
        (Some(_), Some(_)) => return Err("name one point to go back to: `version` or `run`, not both".into()),
        (None, None) => return Err("name the point to go back to: `version` (a version's number) or `run` (a run's), as `history` shows them".into()),
    };
    if !code && !state {
        return Err("say what it would restore: `code`, `state`, or both".into());
    }
    let mut body = json!({ "code": code, "state": state, "dry_run": true });
    for (k, v) in at.as_object().into_iter().flatten() {
        body[k] = v.clone();
    }
    let preview = json_of(api, "POST", &format!("/deployments/{}/restore", seg(slug)), Some(Body::Json(body)))?;
    let strings = |v: &Value| -> Vec<String> { v.as_array().into_iter().flatten().filter_map(|i| i.as_str().map(str::to_string)).collect() };
    let listed = |items: &[String]| items.iter().map(|i| format!("  - {}", agent::describe(i))).collect::<Vec<_>>().join("\n");
    let mut out = vec![format!("Dry run: nothing changed. Going back to {point} of \"{slug}\":")];
    if let Some(why) = preview["refused"].as_str() {
        out.push(format!("Its code can't be deployed on {} now: {why}", api.server()));
    }
    match preview["code"].as_u64() {
        None if code => out.push(format!("{point} ran on the code deployed now: there is no code to deploy again, only its state.")),
        None => {}
        Some(n) => {
            let needs = strings(&preview["needs_approval"]);
            out.push(if needs.is_empty() {
                format!("Its code, version {n}'s, would deploy again as a new version, under the grant it has now.")
            } else {
                format!("Its code, version {n}'s, would deploy again once a person approves what today's grant doesn't cover:\n{}", listed(&needs))
            });
            out.push(code_diff(api, slug, n).unwrap_or_else(|e| format!("Its text can't be compared: {e}")));
        }
    }
    // Everything it touches, beside what it touches now: its secrets not set among them.
    let touches = crate::touches::text(&crate::touches::lines(&preview["touches"], &preview["settings"]));
    if !touches.is_empty() {
        out.push(format!("What it touches (+ new, - no longer, ~ changed, ! not ready on {}):\n{}", api.server(), touches.join("\n")));
    }
    if preview["state"].is_object() {
        out.push(crate::restore::state_lines(&preview["state"]));
        out.extend(state_values(&preview["state"]));
    }
    let what = match (code, state) {
        (true, true) => "--code --state",
        (true, false) => "--code",
        _ => "--state",
    };
    if flag("ask") {
        let mut body = json!({ "code": code, "state": state });
        for (k, v) in at.as_object().into_iter().flatten() {
            body[k] = v.clone();
        }
        let asked = json_of(api, "POST", &format!("/deployments/{}/restore", seg(slug)), Some(Body::Json(body)))?;
        let event = asked["event"].as_u64().map_or(String::new(), |n| format!(" (event #{n})"));
        out.push(format!(
            "\nSaved for a person to approve{event}: it {}. They see it in the editor's history beside the runs, where restoring it is one click, or run `holon restore {slug} {to} {what}`.",
            asked["what"].as_str().unwrap_or("asks to restore it")
        ));
        return Ok(out.join("\n"));
    }
    out.push(format!("\nA person restores it: in the editor, ↺ on that run or in the history beside the runs, or `holon restore {slug} {to} {what}`. With `ask`, this saves it for them to approve."));
    Ok(out.join("\n"))
}

/// The state's keys and collection items a restore would put back, now → then.
pub(crate) fn state_values(change: &Value) -> Vec<String> {
    let line = |name: String, v: &Value| format!("  {name}: {} → {}", agent::show(&v["now"]), agent::show(&v["then"]));
    let mut out: Vec<String> = change["values"].as_object().into_iter().flatten().map(|(k, v)| line(k.clone(), v)).collect();
    for (name, c) in change["collections"].as_object().into_iter().flatten() {
        out.extend(c["items"].as_object().into_iter().flatten().map(|(key, v)| line(format!("{name}[{}]", Value::from(key.as_str())), v)));
        if c["more"] == true {
            out.push(format!("  {name}: and more items"));
        }
    }
    if !out.is_empty() {
        out.insert(0, "Now → as it would be put back:".into());
    }
    out
}

/// Version `number`'s text beside the text deployed now, as a line diff. Both with the text's
/// names, or both printed plainly when only one has them (a canvas deploy keeps none), so a name
/// alone never reads as a change.
/// The diff of version `number`'s text against the text deployed now, one line each, as `code_diff`
/// prints it, and the note that says when the texts had to be printed without their names.
pub(crate) fn code_diff_lines(api: &dyn Api, slug: &str, number: u64) -> Result<(Vec<String>, &'static str), String> {
    let (now_text, then_text, note) = code_texts(api, slug, number)?;
    Ok((line_diff(&now_text, &then_text), note))
}

pub(crate) fn code_diff(api: &dyn Api, slug: &str, number: u64) -> Result<String, String> {
    let (now_text, then_text, note) = code_texts(api, slug, number)?;
    let lines = line_diff(&now_text, &then_text);
    if lines.is_empty() {
        return Ok(format!("Version {number}'s text is the text deployed now."));
    }
    Ok(format!("Its text (- deployed now, + version {number}'s):\n{}{note}", lines.join("\n")))
}

/// The text deployed now and version `number`'s, printed alike, to compare; and a note when
/// they had to be printed without their names.
pub(crate) fn code_texts(api: &dyn Api, slug: &str, number: u64) -> Result<(String, String, &'static str), String> {
    let path = format!("/deployments/{}", seg(slug));
    let versions = json_of(api, "GET", &format!("{path}/versions"), None)?;
    let unit = versions["versions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| v["number"].as_u64() == Some(number))
        .and_then(|v| v["unit"].as_str())
        .ok_or_else(|| format!("version {number} isn't listed"))?
        .to_string();
    let now = json_of(api, "GET", &format!("{path}/document"), None)?;
    let then = json_of(api, "GET", &format!("{path}/document?unit={unit}"), None)?;
    let named = |d: &Value| d.get("source").and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty()) || d.get("bindings").and_then(Value::as_object).is_some_and(|b| !b.is_empty());
    let plain = |d: &Value| {
        let mut d = d.clone();
        if let Some(o) = d.as_object_mut() {
            o.remove("source");
            o.remove("bindings");
        }
        d
    };
    Ok(if named(&now) == named(&then) {
        (agent::text_of(slug, &now)?, agent::text_of(slug, &then)?, "")
    } else {
        (agent::text_of(slug, &plain(&now))?, agent::text_of(slug, &plain(&then))?, "\n(Both printed without the text's names: one of them was deployed from the canvas, which keeps none.)")
    })
}

/// A line diff of `a` to `b`: `- ` for a's lines, `+ ` for b's, `  ` for both, with the unchanged
/// lines more than two away from a change cut to `  …`. Empty when they're the same.
fn line_diff(a: &str, b: &str) -> Vec<String> {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    if a.len() * b.len() > 4_000_000 {
        return vec![format!("  (too long to compare line by line: {} lines and {})", a.len(), b.len())];
    }
    // common[i][j]: the most lines a[i..] and b[j..] have in common, in order.
    let mut common = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            common[i][j] = if a[i] == b[j] { common[i + 1][j + 1] + 1 } else { common[i + 1][j].max(common[i][j + 1]) };
        }
    }
    let mut all: Vec<(char, &str)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            all.push((' ', a[i]));
            (i, j) = (i + 1, j + 1);
        } else if i < a.len() && (j == b.len() || common[i + 1][j] >= common[i][j + 1]) {
            all.push(('-', a[i]));
            i += 1;
        } else {
            all.push(('+', b[j]));
            j += 1;
        }
    }
    if all.iter().all(|(sign, _)| *sign == ' ') {
        return Vec::new();
    }
    let near = |k: usize| all[k.saturating_sub(2)..(k + 3).min(all.len())].iter().any(|(sign, _)| *sign != ' ');
    let mut out: Vec<String> = Vec::new();
    for (k, (sign, line)) in all.iter().enumerate() {
        if *sign != ' ' || near(k) {
            out.push(format!("{sign} {line}"));
        } else if out.last().map(String::as_str) != Some("  …") {
            out.push("  …".into());
        }
    }
    out
}

// ── the protocol ────────────────────────────────────────────────────────────────────────────

/// What a connection has done so far. The protocol's lifecycle starts with `initialize`: until the
/// client has sent it, only `ping` is answered.
#[derive(Default)]
pub(crate) struct Session {
    initialized: bool,
}

/// What is wrong with a call's arguments by the tool's own schema, said to the agent that sent
/// them: one of the wrong kind is told as that, and not as one that is missing (the model that
/// sent `text: 5` can then correct itself). None when they are what the tool takes. An argument
/// the tool doesn't list is let through, as a client may send more than a server knows.
fn invalid_arguments(name: &str, args: &Value) -> Option<String> {
    fn kind(v: &Value) -> &'static str {
        match v {
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Number(_) => "a number",
            Value::String(_) => "a text",
            Value::Array(_) => "a list",
            Value::Object(_) => "an object",
        }
    }
    let tools = tool_list();
    let schema = tools.as_array()?.iter().find(|t| t["name"] == name)?.get("inputSchema")?;
    let Some(given) = args.as_object() else {
        return Some(format!("the arguments of `{name}` must be an object, not {}", kind(args)));
    };
    let properties = schema.get("properties").and_then(Value::as_object)?;
    for (key, value) in given {
        let Some(spec) = properties.get(key) else { continue };
        let fits = match spec.get("type").and_then(Value::as_str) {
            Some("string") => value.is_string(),
            Some("number" | "integer") => value.is_number(),
            Some("boolean") => value.is_boolean(),
            Some("object") => value.is_object(),
            Some("array") => value.is_array(),
            _ => true,
        };
        if !fits {
            let wants = match spec["type"].as_str() {
                Some("string") => "a text",
                Some("number" | "integer") => "a number",
                Some("boolean") => "true or false",
                Some("object") => "an object",
                _ => "a list",
            };
            return Some(format!("`{key}` must be {wants}, not {}", kind(value)));
        }
        if let Some(options) = spec.get("enum").and_then(Value::as_array)
            && !options.contains(value)
        {
            let listed: Vec<String> = options.iter().map(|o| o.to_string()).collect();
            return Some(format!("`{key}` must be one of {}, not {value}", listed.join(", ")));
        }
    }
    None
}

/// A tool call the connection has accepted and will answer when it is done, on a thread of its
/// own: a call can take a while, and the others (a `ping`, a cancel) are not made to wait for it.
pub(crate) struct Call {
    id: Value,
    name: String,
    args: Value,
    /// The request's progress token, when it asked to be told how the call goes.
    progress_token: Option<Value>,
}

/// The answer to one JSON-RPC message, now: none for a notification, and none for a tool call,
/// which is handed to `dispatch` to answer itself.
pub(crate) fn handle(message: &Value, session: &mut Session, dispatch: &mut dyn FnMut(Call)) -> Option<Value> {
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
    if !session.initialized && method != "initialize" && method != "ping" {
        return reply(Err((-32600, format!("send `initialize` before `{method}`"))));
    }
    match method {
        "initialize" => {
            let asked = message.pointer("/params/protocolVersion").and_then(Value::as_str);
            let version = asked.filter(|v| VERSIONS.contains(v)).unwrap_or(VERSIONS[VERSIONS.len() - 1]);
            session.initialized = true;
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
            // A tool's failure, not the protocol's: the agent reads it and tries again.
            if let Some(why) = invalid_arguments(name, &args) {
                return reply(Ok(json!({ "content": [{ "type": "text", "text": why }], "isError": true })));
            }
            dispatch(Call { id: id.clone().unwrap_or(Value::Null), name: name.to_string(), args, progress_token: message.pointer("/params/_meta/progressToken").cloned() });
            None
        }
        other => reply(Err((-32601, format!("no method \"{other}\"")))),
    }
}

/// How long calls still running are given once the input has ended.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// One accepted call, run: its answer, said when it is done, unless whoever asked has cancelled
/// it (the protocol asks that a cancelled request is not answered).
fn answer_call(call: Call, cancelled: &AtomicBool, connect: &Connect, say: &(dyn Fn(&Value) + Sync)) {
    let Call { id, name, args, progress_token } = call;
    let progress = |done: u64, of: u64, note: &str| {
        if let Some(token) = &progress_token {
            say(&json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": { "progressToken": token, "progress": done, "total": of, "message": note } }));
        }
    };
    let ctx = Ctx { cancelled: Some(cancelled), progress: progress_token.is_some().then_some(&progress as &Progress<'_>) };
    let (text, failed) = match call_tool(&name, &args, connect, &ctx) {
        Ok(t) => (t, false),
        Err(e) => (e, true),
    };
    if !cancelled.load(Ordering::Relaxed) {
        say(&json!({ "jsonrpc": "2.0", "id": id, "result": { "content": [{ "type": "text", "text": text }], "isError": failed } }));
    }
}

/// Serve the protocol on `input` and `output` until the input ends. What is read, and what is
/// quick, is answered as it comes; a tool call runs on a thread of its own and answers when it
/// is done, so a call that waits on a person doesn't hold up a `ping` or the cancel of itself.
/// When the input ends, calls still running are cancelled and awaited.
pub(crate) fn serve<R: BufRead, W: Write + Send>(input: R, output: W, connect: &Connect) -> std::io::Result<()> {
    let out = Mutex::new(output);
    // One line at a time, whole, whichever thread says it.
    let say = |v: &Value| {
        let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(out, "{v}").and_then(|()| out.flush());
    };
    // The calls running, by request id, each with the flag that cancels it.
    let running: Mutex<HashMap<String, Arc<AtomicBool>>> = Mutex::new(HashMap::new());
    let mut session = Session::default();
    std::thread::scope(|scope| -> std::io::Result<()> {
        let mut dispatch = |call: Call| {
            let cancelled = Arc::new(AtomicBool::new(false));
            let key = call.id.to_string();
            running.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone(), cancelled.clone());
            let (say, running) = (&say, &running);
            scope.spawn(move || {
                answer_call(call, &cancelled, connect, say);
                running.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
            });
        };
        let read = || -> std::io::Result<()> {
            for line in input.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let answer = match serde_json::from_str::<Value>(&line) {
                    Ok(message) if message.get("method").and_then(Value::as_str) == Some("notifications/cancelled") => {
                        // Who asked has given up on a request: stop it, and say nothing of it.
                        if let Some(id) = message.pointer("/params/requestId")
                            && let Some(flag) = running.lock().unwrap_or_else(|e| e.into_inner()).get(&id.to_string())
                        {
                            flag.store(true, Ordering::Relaxed);
                        }
                        None
                    }
                    Ok(message) if message.is_object() => handle(&message, &mut session, &mut dispatch),
                    Ok(_) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32600, "message": "one JSON-RPC message per line, not a batch" } })),
                    Err(e) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("not JSON: {e}") } })),
                };
                if let Some(a) = answer {
                    say(&a);
                }
            }
            Ok(())
        };
        let ended = read();
        // The input has ended, or failed. What a client sent before it closed (a script that pipes
        // its requests in) is answered if it takes no longer than a moment; what waits longer is
        // cancelled, as nobody is left to read it.
        let grace = std::time::Instant::now() + SHUTDOWN_GRACE;
        while std::time::Instant::now() < grace && !running.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        for flag in running.lock().unwrap_or_else(|e| e.into_inner()).values() {
            flag.store(true, Ordering::Relaxed);
        }
        ended
    })
}

/// The servers and tokens `holon mcp` has found to sign in as an agent, by the server and the
/// token's hash: `/auth/me` is asked once a process, not once a tool call.
type Checked = Mutex<HashSet<(String, String)>>;

/// The server `holon mcp` reaches, signed in as an agent (local::agent_target). On a server this
/// user runs with no agent's token kept, the run file's admin makes theirs first
/// (mcphosts::local_agent), and again once if the one kept isn't taken (a new accounts file).
/// A token that signs in as anyone but an agent is refused, never signed in with.
fn agent_http(given: Option<&str>, checked: &Checked) -> Result<Http, String> {
    let target = local::agent_target(given)?;
    let server = target.server.as_str();
    let make = || -> Result<String, String> {
        let Some(run) = target.run_token.as_deref() else {
            return Err(format!("holon mcp signs in to {server} as an agent, and has no agent's token for it: holon mcp setup --server {server} keeps one (or set HOLON_TOKEN to one)"));
        };
        let file = local::tokens_file().ok_or("no home folder to keep the agent's token in")?;
        Ok(crate::mcphosts::local_agent(&Http::new(server, Some(run)), None, &file)?.2)
    };
    // Held while a token is checked or made, so calls at once make one token, not one each.
    let mut checked = checked.lock().unwrap_or_else(|e| e.into_inner());
    let mut token = match &target.token {
        Some(t) => t.clone(),
        None => make()?,
    };
    let key = |token: &str| (server.to_string(), blake3::hash(token.as_bytes()).to_hex().to_string());
    if checked.contains(&key(&token)) {
        return Ok(Http::new(server, Some(&token)));
    }
    let mut me = crate::mcphosts::who(&Http::new(server, Some(&token)))?;
    if me == Some(crate::mcphosts::Me::NotTaken) && target.run_token.is_some() {
        token = make()?;
        me = crate::mcphosts::who(&Http::new(server, Some(&token)))?;
    }
    // Not reached, it isn't checked yet: the tool's own request says so.
    if let Some(me) = me {
        crate::mcphosts::an_agent(&me, server)?;
        checked.insert(key(&token));
    }
    Ok(Http::new(server, Some(&token)))
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
    let checked = Checked::default();
    let connect = || -> Result<Box<dyn Api>, String> { Ok(Box::new(agent_http(server.as_deref(), &checked)?)) };
    eprintln!("holon mcp: ready on stdin and stdout");
    match serve(std::io::stdin().lock(), std::io::stdout(), &connect) {
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
    use crate::tests::{body_bytes, req_as, test_state, with_people, Signed};
    use crate::{route, AppState};

    /// `deploy`, with nothing said for the person who reviews it.
    fn deploy(api: &dyn Api, text: &str, slug: Option<&str>) -> Result<String, String> {
        super::deploy(api, text, slug, Asked::default())
    }

    /// Call the tool `name` as `token` signs in.
    fn tool(state: &AppState, token: &str, name: &str, args: Value) -> Result<String, String> {
        let connect = || -> Result<Box<dyn Api + '_>, String> { Ok(Box::new(Signed { state, token })) };
        call_tool(name, &args, &connect, &Ctx::default())
    }

    const SLUGS_TEXT: &str = include_str!("../../holon-author/samples/slugs.holon.ts");

    #[test]
    fn whoami_says_what_each_binding_allows_and_where() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let said = tool(&state, &p.claude, "whoami", json!({})).unwrap();
        assert!(said.starts_with("You are claude, an agent, owned by ada, on the test server.\nYou hold:\n  - agent on this host: flow.view, "), "{said}");
        assert!(said.ends_with("and reads and changes people, roles and resources."), "{said}");
        // Bound more, through a group, until a time: each its own line.
        let ok = |method: &str, path: &str, body: Value| assert_eq!(route(&state, &req_as(&p.ada, method, path, body.to_string().into_bytes())).status_code, 200, "{path}");
        ok("PUT", "/groups/bots", json!({ "members": ["claude"] }));
        ok("POST", "/bindings", json!({ "who": { "group": "bots" }, "role": "viewer", "scope": { "flows": { "slug": "payout" } }, "expires_at": 4_000_000_000u64 }));
        let said = tool(&state, &p.claude, "whoami", json!({})).unwrap();
        assert!(said.contains("\n  - viewer on \"payout\" (through the group bots), until 2096-10-02T07:06:40: flow.view, "), "{said}");
    }

    #[test]
    fn a_flow_goes_from_text_to_an_explained_run_and_the_agent_never_approves() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let text = SLUGS_TEXT;
        // Checked without a server.
        let checked = tool(&state, &p.claude, "check", json!({ "text": text })).unwrap();
        assert!(checked.contains("POST /slugs") && checked.contains("it may:"), "{checked}");
        // A first deploy waits for a person: saved, not deployed, and it says what they approve.
        let first = tool(&state, &p.claude, "deploy", json!({ "text": text })).unwrap();
        assert!(first.starts_with("Not deployed yet"), "{first}");
        assert!(first.contains("receive requests") && first.contains("\"slugs\""), "{first}");
        let saved = serde_json::from_slice::<Value>(&body_bytes(route(&state, &req_as(&p.ada, "GET", "/flows/slugs", vec![])))).unwrap();
        assert!(saved["document"]["trace"].is_object(), "the saved flow is the document a deploy sends");
        assert!(serde_json::from_slice::<Value>(&body_bytes(route(&state, &req_as(&p.ada, "GET", "/deployments", vec![])))).unwrap()["deployments"].as_array().unwrap().is_empty());
        // A person approves it (the editor's deploy: everything the preview asked).
        let plan = agent::plan(text).unwrap();
        let preview: Value = serde_json::from_slice(&body_bytes(route(&state, &req_as(&p.ada, "POST", "/deploy/preview", serde_json::to_vec(&json!({ "slug": "slugs", "unit": plan.unit })).unwrap())))).unwrap();
        let approve = json!({ "slug": "slugs", "unit": plan.unit, "acknowledge": preview["needs_approval"], "document": plan.document });
        assert_eq!(route(&state, &req_as(&p.ada, "POST", "/deploy", serde_json::to_vec(&approve).unwrap())).status_code, 200);
        // A new version that needs nothing more deploys from the agent at once.
        let v2 = text.replace("\"unsigned\"", "\"not signed\"");
        let second = tool(&state, &p.claude, "deploy", json!({ "text": v2 })).unwrap();
        assert!(second.starts_with("Deployed \"slugs\""), "{second}");
        assert!(second.contains("POST /slugs"), "{second}");
        // Called: its answer, and its run.
        let called = tool(&state, &p.claude, "call", json!({ "slug": "slugs", "method": "post", "path": "/slugs", "body": { "title": "Hello World", "author": "Ada" } })).unwrap();
        assert!(called.starts_with("POST /slugs → 200"), "{called}");
        assert!(called.contains("hello-world") && called.contains("not signed"), "{called}");
        assert!(called.contains("This was run"), "{called}");
        // One that fails at a missing field, explained: the statement, and where the value went missing.
        let failed = tool(&state, &p.claude, "call", json!({ "slug": "slugs", "method": "POST", "path": "/slugs", "body": { "title": "only a title" } })).unwrap();
        assert!(failed.starts_with("POST /slugs → 400"), "{failed}");
        let listed = tool(&state, &p.claude, "runs", json!({ "slug": "slugs" })).unwrap();
        assert_eq!(listed.lines().count(), 2, "{listed}");
        assert!(listed.lines().nth(1).unwrap().contains("POST /slugs → 400"), "{listed}");
        let why = tool(&state, &p.claude, "explain", json!({ "slug": "slugs" })).unwrap();
        assert!(why.contains("author = slugify(req.json.author)"), "the agent's own names: {why}");
        assert!(why.contains("⇒ nothing: field \"author\" gave nothing"), "{why}");
        assert!(why.contains("⇒ \"only-a-title\""), "{why}");
        assert!(why.contains("⇒ req.json = { title: \"only a title\" }"), "the request, as it arrived: {why}");
        assert!(!why.contains("didn't run"), "the other route is shown without values: {why}");
        let first_run = called.split("This was run ").nth(1).unwrap().split(':').next().unwrap().parse::<u64>().unwrap();
        let ok = tool(&state, &p.claude, "explain", json!({ "slug": "slugs", "run": first_run })).unwrap();
        assert!(ok.contains("⇒ \"ada\"") && ok.contains("answered 200"), "{ok}");
        assert!(ok.contains("performed"), "its writes: {ok}");
        // What it keeps: its state, and its collection's items.
        let kept = tool(&state, &p.claude, "state", json!({ "slug": "slugs" })).unwrap();
        assert!(kept.contains("last_post  \"only-a-title\""), "the latest run's: {kept}");
        assert!(kept.contains("posts") && kept.contains("`state` with collection posts"), "{kept}");
        let posts = tool(&state, &p.claude, "state", json!({ "slug": "slugs", "collection": "posts" })).unwrap();
        assert!(posts.starts_with("\"hello-world\" → { at: ") && posts.contains("author: \"ada\""), "{posts}");
    }

    /// `deployments` with `wait` holds for a person's decision, and no longer than asked: a server
    /// that answers its proposals as a script says (waiting, waiting, declined).
    #[test]
    fn deployments_waits_for_a_proposal_to_be_decided_and_says_when_it_is_still_waiting() {
        use std::cell::Cell;
        struct Proposals<'a> {
            asked: Cell<usize>,
            states: &'a [&'a str],
        }
        impl Api for Proposals<'_> {
            fn server(&self) -> String {
                "the script".into()
            }
            fn send(&self, _: &str, path: &str, _: Option<Body>) -> Result<(u16, Vec<u8>), String> {
                assert_eq!(path, "/proposals");
                let at = self.asked.get().min(self.states.len() - 1);
                self.asked.set(self.asked.get() + 1);
                Ok((200, json!({ "proposals": [{ "id": 1, "slug": "tide", "state": self.states[at] }, { "id": 2, "slug": "other", "state": "waiting" }] }).to_string().into_bytes()))
            }
        }
        let tick = Duration::from_millis(5);
        // Decided on the third look: the call returns then, and says so.
        let api = Proposals { asked: Cell::new(0), states: &["waiting", "waiting", "declined"] };
        let said = wait_for_decision(&api, "tide", 10, tick, &Ctx::default()).unwrap();
        assert!(said.starts_with("A person decided while you waited"), "{said}");
        assert_eq!(api.asked.get(), 3, "it looked until it was decided, and no more");
        // Never decided: it stops at the time asked, and says how to go on.
        let api = Proposals { asked: Cell::new(0), states: &["waiting"] };
        let started = std::time::Instant::now();
        let said = wait_for_decision(&api, "tide", 1, tick, &Ctx::default()).unwrap();
        assert!(said.starts_with("Still waiting for a person after 1 seconds") && said.contains("`wait`"), "{said}");
        assert!(started.elapsed() < Duration::from_secs(3));
        // Nothing waits: no wait, and nothing to say before the answer.
        let api = Proposals { asked: Cell::new(0), states: &["approved"] };
        assert_eq!(wait_for_decision(&api, "tide", 10, tick, &Ctx::default()), None);
        assert_eq!(api.asked.get(), 1);
        // Another flow's proposal waiting is not this one's.
        assert_eq!(wait_for_decision(&Proposals { asked: Cell::new(0), states: &["declined"] }, "other-flow", 10, tick, &Ctx::default()), None);
    }

    /// A flow's `browsers` line reaches the server as the list its deploy request takes, so a
    /// flow that answers web pages deploys from the agent like any other.
    #[test]
    fn a_flow_that_answers_browsers_deploys_with_them_as_a_list() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let text = "import { flow, route } from \"holon\"\n\nflow(\"page-api\", { port: 8199, browsers: [\"https://app.example\", \"*\"] })\n\nroute.get(\"/hi\", (req) => {\n  const name = req.query.name ?? \"you\"\n  return `hi ${name}`\n})\n";
        let first = tool(&state, &p.claude, "deploy", json!({ "text": text })).unwrap();
        assert!(first.starts_with("Not deployed yet"), "{first}");
        let plan = agent::plan(text).unwrap();
        assert_eq!(plan.browsers.as_deref(), Some("https://app.example *"), "the plan keeps them on one line");
        let preview: Value = serde_json::from_slice(&body_bytes(route(&state, &req_as(&p.ada, "POST", "/deploy/preview", serde_json::to_vec(&json!({ "slug": "page-api", "unit": plan.unit })).unwrap())))).unwrap();
        let approve = json!({ "slug": "page-api", "unit": plan.unit, "acknowledge": preview["needs_approval"], "document": plan.document, "browsers": ["https://app.example", "*"] });
        assert_eq!(route(&state, &req_as(&p.ada, "POST", "/deploy", serde_json::to_vec(&approve).unwrap())).status_code, 200);
        // The next version deploys from the agent, with the origins one by one.
        let v2 = text.replace("\"hi {name}\"", "\"hello {name}\"");
        let second = tool(&state, &p.claude, "deploy", json!({ "text": v2 })).unwrap();
        assert!(second.starts_with("Deployed \"page-api\""), "{second}");
        let deployments: Value = serde_json::from_slice(&body_bytes(route(&state, &req_as(&p.ada, "GET", "/deployments", vec![])))).unwrap();
        let entry = deployments["deployments"].as_array().unwrap().iter().find(|d| d["slug"] == "page-api").cloned().unwrap_or(Value::Null);
        assert_eq!(entry["browsers"], json!(["https://app.example", "*"]), "{entry}");
    }

    #[test]
    fn refusals_and_mistakes_come_back_as_failed_tools_in_sentences() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let e = tool(&state, &p.claude, "check", json!({ "text": "import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.get(\"/a\", () => nope)\n" })).unwrap_err();
        assert!(e.contains("text:5:23: unknown name \"nope\"") && e.contains('^'), "{e}");
        assert!(tool(&state, &p.claude, "deploy", json!({ "text": "import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.get(\"/a\", () => nope)\n" })).is_err());
        assert!(tool(&state, &p.claude, "explain", json!({ "slug": "nobody" })).unwrap_err().contains("404"));
        assert!(tool(&state, &p.claude, "check", json!({})).unwrap_err().contains("`text` is missing"));
    }

    /// An agent, signed in with its own token, proposes and calls, under its own name; the server
    /// refuses it the rest, whatever it sends.
    #[test]
    fn an_agent_only_proposes() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let (agent, person) = (Signed { state: &state, token: &p.claude }, Signed { state: &state, token: &p.ada });
        let slug = Some("agent-slugs");
        assert!(deploy(&agent, SLUGS_TEXT, slug).unwrap().starts_with("Not deployed yet"));
        assert_eq!(json_of(&person, "GET", "/flows/agent-slugs", None).unwrap()["saved_by"], "claude", "proposed under its own name");
        // Approving it itself is refused; deploying what needs approval proposes it.
        let plan = agent::plan(SLUGS_TEXT).unwrap();
        let ask = |acknowledge: Value| Some(Body::Json(json!({ "slug": "agent-slugs", "unit": plan.unit, "acknowledge": acknowledge, "document": plan.document })));
        let approved = json_of(&agent, "POST", "/deploy", ask(json!(["NetworkIngress"]))).unwrap_err();
        assert!(approved.contains("refused") && approved.contains("an agent doesn't approve"), "{approved}");
        let unasked = json_of(&agent, "POST", "/deploy", ask(json!([]))).unwrap();
        assert_eq!(unasked["proposal"]["state"], "waiting", "{unasked}");
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
        assert!(listed.contains("sent by claude"), "the run names who sent it: {listed}");
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
            ("GET", "/people"),
        ] {
            assert_eq!(agent.send(method, path, None).unwrap().0, 403, "{method} {path}");
        }
        // The server, too, keeps it from replacing a flow the person saved.
        json_of(&person, "POST", "/flows", Some(Body::Json(json!({ "name": "mine", "document": {} })))).unwrap();
        let replaced = json_of(&agent, "POST", "/flows", Some(Body::Json(json!({ "name": "mine", "document": {} })))).unwrap_err();
        assert!(replaced.contains("saved by ada") && replaced.contains("doesn't replace"), "{replaced}");
        assert_eq!(person.send("POST", "/deployments/agent-slugs/stop", None).unwrap().0, 200);
    }

    /// An agent reads a deployment's history, and sees what going back would change: the code's
    /// text beside today's, the prices put back. It restores nothing: a person does.
    #[test]
    fn an_agent_reads_the_history_and_sees_a_restore_as_a_dry_run_only() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let (agent, person) = (Signed { state: &state, token: &p.claude }, Signed { state: &state, token: &p.ada });
        let text = include_str!("../../holon-author/samples/price-rollback.holon.ts").replace("port: 8160", "port: 18170");
        // Proposed by the agent (its unit stored), approved by the person.
        assert!(deploy(&agent, &text, None).unwrap().starts_with("Not deployed yet"));
        let plan = agent::plan(&text).unwrap();
        let preview = json_of(&person, "POST", "/deploy/preview", Some(Body::Json(json!({ "slug": "price-rollback", "unit": plan.unit })))).unwrap();
        json_of(&person, "POST", "/deploy", Some(Body::Json(json!({ "slug": "price-rollback", "unit": plan.unit, "acknowledge": preview["needs_approval"], "document": plan.document })))).unwrap();
        // Version 2, with another fee, from the agent: it needs nothing more.
        assert!(deploy(&agent, &text.replace("fee = 0.5", "fee = 5"), None).unwrap().starts_with("Deployed \"price-rollback\""));
        let put = |price: u64| call(&agent, &json!({ "slug": "price-rollback", "method": "PUT", "path": "/prices/tea", "body": { "price": price } })).unwrap();
        let first = put(4);
        put(40);
        let first = first.split("This was run ").nth(1).unwrap().split(':').next().unwrap().parse::<u64>().unwrap();

        let history = tool_as(&agent, "history", json!({ "slug": "price-rollback" })).unwrap();
        assert!(history.contains("deployed version 1") && history.contains("deployed version 2"), "{history}");
        assert!(history.contains(&format!("#{first}")) && history.contains("PUT /prices/tea  200  v2"), "{history}");
        assert!(history.find("deployed version 2").unwrap() < history.find(&format!("#{first}")).unwrap(), "oldest first: {history}");
        let events = tool_as(&agent, "history", json!({ "slug": "price-rollback", "events": true })).unwrap();
        assert!(!events.contains("PUT /prices"), "events only: {events}");
        // One item's history: the two runs that priced tea, each with the price before and after.
        let tea = tool_as(&agent, "history", json!({ "slug": "price-rollback", "cell": "prices", "item": "tea" })).unwrap();
        assert!(tea.starts_with("prices[\"tea\"], oldest first, now 40:"), "{tea}");
        assert!(tea.contains(&format!("#{first}")) && tea.contains("nothing → 4") && tea.contains("4 → 40"), "{tea}");
        assert!(tool_as(&agent, "history", json!({ "slug": "price-rollback", "cell": "fee" })).unwrap().starts_with("state.fee: no run in the log changed it"));

        // Version 1's code: its text beside today's, the fee the one line changed.
        let code = tool_as(&agent, "restore", json!({ "slug": "price-rollback", "version": 1, "code": true })).unwrap();
        assert!(code.starts_with("Dry run: nothing changed. Going back to version 1"), "{code}");
        assert!(code.contains("version 1's, would deploy again as a new version"), "{code}");
        assert!(code.contains("-   const fee = 5\n+   const fee = 0.5"), "{code}");
        assert!(code.contains("holon restore price-rollback --to 1 --code"), "{code}");
        // The first PUT's state: tea back to 4, and the run since, which did nothing outside.
        let put_back = tool_as(&agent, "restore", json!({ "slug": "price-rollback", "run": first, "state": true })).unwrap();
        assert!(put_back.contains("prices[\"tea\"]: 40 → 4"), "{put_back}");
        assert!(put_back.contains(&format!("1 run(s) logged since run {first}")), "{put_back}");
        assert!(tool_as(&agent, "restore", json!({ "slug": "price-rollback", "run": first })).unwrap_err().contains("`code`, `state`, or both"));

        // Nothing changed. Asked, the restore is saved for a person: an event on the timeline, the
        // state as it was.
        let restore = |dry_run: bool| Some(Body::Json(json!({ "run": first, "state": true, "dry_run": dry_run })));
        let asked = tool_as(&agent, "restore", json!({ "slug": "price-rollback", "run": first, "state": true, "ask": true })).unwrap();
        assert!(asked.contains("Saved for a person to approve (event #") && asked.contains(&format!("it asks to put back the state run {first} left")), "{asked}");
        let quote = call(&agent, &json!({ "slug": "price-rollback", "method": "GET", "path": "/quote/tea" })).unwrap();
        assert!(quote.contains("\"price\":40") && quote.contains("\"fee\":5"), "{quote}");
        let timeline = tool_as(&agent, "history", json!({ "slug": "price-rollback", "events": true })).unwrap();
        assert!(timeline.contains(&format!("asks to put back the state run {first} left")), "{timeline}");
        assert!(json_of(&person, "POST", "/deployments/price-rollback/restore", restore(false)).unwrap()["state"]["session"].is_u64(), "a person restores it");
        assert_eq!(person.send("POST", "/deployments/price-rollback/stop", None).unwrap().0, 200);
    }

    /// An agent runs a flow's action only as its grant lets agents: a person asks for it (the
    /// deploy settings' `agents`) and approves `AgentRuns("reset")`; taking it back refuses the
    /// agent again and leaves the flow running. A person runs it either way.
    #[test]
    fn an_agent_runs_an_action_only_as_its_grant_lets_agents() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let (agent, person) = (Signed { state: &state, token: &p.claude }, Signed { state: &state, token: &p.ada });
        let text = include_str!("../../holon-author/samples/ticker.holon.ts").replace("port: 8156", "port: 18259");
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
        assert!(runs(&agent, "ticker", 5).unwrap().contains("claude"), "the run names who ran it");

        json_of(&person, "POST", "/deployments/ticker/grant", Some(Body::Json(json!({ "revoke": ["AgentRuns(\"reset\")"] })))).unwrap();
        assert!(tool_as(&agent, "run", json!({ "slug": "ticker", "action": "reset" })).unwrap_err().contains("agents may not run"));
        assert!(state.deployments.lock().unwrap()["ticker"].life.status == crate::lifecycle::Status::Running, "taken back, it runs on");
        assert!(deploy_with(json!(["nope"])).unwrap_err().contains("it has no action \"nope\""));
        assert_eq!(person.send("POST", "/deployments/ticker/stop", None).unwrap().0, 200);
    }

    /// An agent reads a flow's log lines only as its grant lets agents: a person asks for it (the deploy
    /// settings' `agent_logs`) and approves `AgentReadsLogs`; taking it back refuses the agent again and
    /// leaves the flow running. A person reads them either way.
    #[test]
    fn an_agent_reads_log_lines_only_as_its_grant_lets_agents() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let (agent, person) = (Signed { state: &state, token: &p.claude }, Signed { state: &state, token: &p.ada });
        let text = include_str!("../../holon-author/samples/traced-payout.holon.ts").replace("port: 8147", "port: 18261");
        let plan = agent::plan(&text).unwrap();
        for unit in &plan.units {
            json_of(&person, "POST", "/unit", Some(Body::Bytes(unit.clone()))).unwrap();
        }
        let deploy_with = |agent_logs: bool| -> Result<Value, String> {
            let requirements = json!({ "agent_logs": agent_logs });
            let preview = json_of(&person, "POST", "/deploy/preview", Some(Body::Json(json!({ "slug": "traced-payout", "unit": plan.unit, "requirements": requirements }))))?;
            let ask = json!({ "slug": "traced-payout", "unit": plan.unit, "acknowledge": preview["needs_approval"], "requirements": requirements, "document": plan.document });
            json_of(&person, "POST", "/deploy", Some(Body::Json(ask)))?;
            Ok(preview)
        };
        let pay = |id: &str, amount: u64| tool_as(&person, "call", json!({ "slug": "traced-payout", "method": "post", "path": "/payout", "body": { "id": id, "amount": amount } })).unwrap();

        // Without the grant, a person reads the lines and an agent is refused them, and told who lets it.
        deploy_with(false).unwrap();
        pay("p-1", 250);
        pay("p-2", 5000);
        let read = tool_as(&person, "logs", json!({ "slug": "traced-payout" })).unwrap();
        assert!(read.contains("payout asked p-1 250") && read.contains("WARN") && read.contains("over the usual limit p-2"), "a person reads them: {read}");
        let refused = tool_as(&agent, "logs", json!({ "slug": "traced-payout" })).unwrap_err();
        assert!(refused.contains("agents may not read") && refused.contains("agents may read its logs"), "{refused}");

        // Asked for, the preview has a person approve it; granted, the agent reads them, filtered as asked.
        let preview = deploy_with(true).unwrap();
        assert!(preview["needs_approval"].as_array().unwrap().contains(&json!("AgentReadsLogs")), "a person approves it: {preview}");
        let read = tool_as(&agent, "logs", json!({ "slug": "traced-payout" })).unwrap();
        assert!(read.contains("payout asked p-1 250") && read.contains("over the usual limit p-2"), "{read}");
        let warned = tool_as(&agent, "logs", json!({ "slug": "traced-payout", "level": "warn" })).unwrap();
        assert!(warned.contains("over the usual limit p-2") && !warned.contains("payout asked"), "{warned}");
        let own = tool_as(&agent, "logs", json!({ "slug": "traced-payout", "run": 0, "own": true })).unwrap();
        assert!(own.contains("payout asked p-1 250") && !own.contains("p-2"), "{own}");

        // Taken back, the agent is refused again, and the flow runs on.
        json_of(&person, "POST", "/deployments/traced-payout/grant", Some(Body::Json(json!({ "revoke": ["AgentReadsLogs"] })))).unwrap();
        assert!(tool_as(&agent, "logs", json!({ "slug": "traced-payout" })).unwrap_err().contains("agents may not read"));
        assert!(state.deployments.lock().unwrap()["traced-payout"].life.status == crate::lifecycle::Status::Running, "taken back, it runs on");
        assert!(tool_as(&person, "logs", json!({ "slug": "traced-payout" })).is_ok(), "a person reads them either way");
        assert_eq!(person.send("POST", "/deployments/traced-payout/stop", None).unwrap().0, 200);
    }

    /// An agent's deploy that asks to keep a handler's runs out of the log stops for a person, who
    /// approves `Unrecorded("GET /hello/:name")` by its words; once deployed, a call to it says its
    /// run was counted, not logged (docs/L4s/request-path-L4.md, C5).
    #[test]
    fn an_agents_deploy_asking_for_unrecorded_stops_for_approval() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let text = include_str!("../../holon-author/samples/hello-api.holon.ts").replace("port: 8101", "port: 18260");
        let approve = |text: &str| {
            let plan = agent::plan(text).unwrap();
            let preview: Value = serde_json::from_slice(&body_bytes(route(&state, &req_as(&p.ada, "POST", "/deploy/preview", serde_json::to_vec(&json!({ "slug": "hello-api", "unit": plan.unit })).unwrap())))).unwrap();
            let approve = json!({ "slug": "hello-api", "unit": plan.unit, "acknowledge": preview["needs_approval"], "document": plan.document });
            assert_eq!(route(&state, &req_as(&p.ada, "POST", "/deploy", serde_json::to_vec(&approve).unwrap())).status_code, 200);
        };
        assert!(tool(&state, &p.claude, "deploy", json!({ "text": text })).unwrap().starts_with("Not deployed yet"));
        approve(&text);
        let unrecorded = text.replace("${req.params.name}`)", r#"${req.params.name}`, { log: "unrecorded" })"#);
        let asked = tool(&state, &p.claude, "deploy", json!({ "text": unrecorded })).unwrap();
        assert!(asked.starts_with("Not deployed yet") && asked.contains("keep runs of GET /hello/:name out of its log"), "{asked}");
        approve(&unrecorded);
        let called = tool(&state, &p.claude, "call", json!({ "slug": "hello-api", "method": "GET", "path": "/hello/Ada" })).unwrap();
        assert!(called.starts_with("GET /hello/Ada → 200"), "{called}");
        assert!(called.contains("Runs of GET /hello/:name aren't recorded"), "{called}");
        let history = tool(&state, &p.claude, "history", json!({ "slug": "hello-api" })).unwrap();
        assert!(history.contains("unrecorded GET /hello/:name  1 run counted, not logged"), "{history}");
        assert_eq!(route(&state, &req_as(&p.ada, "POST", "/deployments/hello-api/stop", vec![])).status_code, 200);
    }

    /// A person let an unrecorded handler's code keep its runs out of the log, not the code an agent
    /// puts in its place: an agent's version that changes another handler keeps it unrecorded; one
    /// that changes its code deploys with its runs logged, `Unrecorded` withheld and offered back to
    /// a person; and the agent's later versions keep it withheld.
    #[test]
    fn an_agent_changing_an_unrecorded_handler_has_its_runs_logged() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let (agent, person) = (Signed { state: &state, token: &p.claude }, Signed { state: &state, token: &p.ada });
        let (text, slug) = ("import { flow, route } from \"holon\"\n\nflow(\"quiet-api\", { port: 18261 })\n\nroute.get(\"/health\", () => \"ok\", { log: \"unrecorded\" })\n\nroute.get(\"/hi\", () => \"hi\")\n", Some("quiet-api"));
        assert!(deploy(&agent, text, slug).unwrap().starts_with("Not deployed yet"));
        let plan = agent::plan(text).unwrap();
        let preview = json_of(&person, "POST", "/deploy/preview", Some(Body::Json(json!({ "slug": "quiet-api", "unit": plan.unit })))).unwrap();
        let approve = json!({ "slug": "quiet-api", "unit": plan.unit, "acknowledge": preview["needs_approval"], "document": plan.document });
        json_of(&person, "POST", "/deploy", Some(Body::Json(approve))).unwrap();
        let item = json!("Unrecorded(\"GET /health\")");
        let grant = || json_of(&person, "GET", "/deployments/quiet-api/grant", None).unwrap()["grant"].clone();
        let deployed = |text: &str| {
            let said = deploy(&agent, text, slug).unwrap();
            assert!(said.starts_with("Deployed \"quiet-api\""), "{said}");
        };
        let health = || call(&agent, &json!({ "slug": "quiet-api", "method": "GET", "path": "/health" })).unwrap();
        assert!(grant()["items"].as_array().unwrap().contains(&item), "{}", grant());

        let other = text.replace("() => \"hi\"", "() => \"hello\"");
        deployed(&other);
        assert!(grant()["items"].as_array().unwrap().contains(&item), "{}", grant());
        let called = health();
        assert!(called.contains("Runs of GET /health aren't recorded"), "{called}");

        let changed = other.replace("() => \"ok\"", "() => \"fine\"");
        deployed(&changed);
        let withheld = grant();
        assert!(!withheld["items"].as_array().unwrap().contains(&item) && withheld["revoked"].as_array().unwrap().contains(&item), "{withheld}");
        let called = health();
        assert!(called.starts_with("GET /health → 200") && !called.contains("aren't recorded"), "{called}");

        deployed(&changed.replace("() => \"hello\"", "() => \"hey\""));
        assert!(grant()["revoked"].as_array().unwrap().contains(&item), "{}", grant());
        assert_eq!(person.send("POST", "/deployments/quiet-api/stop", None).unwrap().0, 200);
    }

    /// An agent replays its new version on the flow's recent runs before it deploys it (G11): how
    /// many it answers the same, and how the rest differ; nothing is deployed, sent or written.
    #[test]
    fn an_agent_replays_its_version_on_the_flows_recent_runs() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let (agent, person) = (Signed { state: &state, token: &p.claude }, Signed { state: &state, token: &p.ada });
        let text = "import { flow, route, status } from \"holon\"\n\nflow(\"price\", { port: 18262 })\n\nroute.post(\"/price\", (req) => {\n  const total = req.json.total ?? 0\n  if (!(total > 0)) {\n    return status(422, \"an order has a total\")\n  }\n  return { total, price: (req.json.member ?? false) ? total * 0.9 : total }\n})\n";
        let replay = |text: &str, runs: Option<u64>| tool_as(&agent, "replay", json!({ "text": text, "runs": runs }));
        assert!(replay(text, None).unwrap_err().contains("no deployment \"price\""), "nothing deployed yet");

        let plan = agent::plan(text).unwrap();
        for u in &plan.units {
            json_of(&person, "POST", "/unit", Some(Body::Bytes(u.clone()))).unwrap();
        }
        let preview = json_of(&person, "POST", "/deploy/preview", Some(Body::Json(json!({ "slug": "price", "unit": plan.unit })))).unwrap();
        json_of(&person, "POST", "/deploy", Some(Body::Json(json!({ "slug": "price", "unit": plan.unit, "acknowledge": preview["needs_approval"], "document": plan.document })))).unwrap();
        assert_eq!(replay(text, None).unwrap(), "\"price\" has no runs yet to replay this version on.");
        for body in [r#"{"total":80,"member":true}"#, r#"{"total":80}"#, "{}"] {
            call(&agent, &json!({ "slug": "price", "method": "POST", "path": "/price", "body": body, "headers": { "content-type": "application/json" } })).unwrap();
        }

        let said = replay(&text.replace("total * 0.9", "total * 0.85"), None).unwrap();
        let lines: Vec<&str> = said.lines().collect();
        assert_eq!(lines[0], "This version replayed on \"price\"'s last 3 runs, offline (nothing sent or written): 3 replayed · 2 same · 1 differs", "{said}");
        assert_eq!(lines[1..], ["  run 0  POST /price  differs: its answer"], "{said}");
        let said = replay(&text.replace("if (!(total > 0))", "if (!(total > 10))"), Some(1)).unwrap();
        assert!(said.ends_with("1 replayed · 1 same"), "the newest alone, refused either way: {said}");
        // A mistake in the text is said, as check says it; nothing was deployed.
        assert!(replay("flow(", None).is_err());
        assert_eq!(json_of(&person, "GET", "/deployments/price/log", None).unwrap()["entries"].as_array().unwrap().len(), 3);
        let versions = json_of(&person, "GET", "/deployments/price/versions", None).unwrap();
        assert_eq!(versions["versions"].as_array().unwrap().len(), 1, "{versions}");
        assert_eq!(person.send("POST", "/deployments/price/stop", None).unwrap().0, 200);
    }

    /// An agent reads which run waits at an approval, and who has approved it so far, to tell the
    /// user; it can't decide one (G12): no tool does, and the route refuses it.
    #[test]
    fn an_agent_reads_who_approved_a_waiting_run_and_never_decides_it() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let accounts = state.access.accounts().unwrap();
        let dan = accounts.accept(&accounts.invite("dan", "editor", None, "ada").unwrap(), "correct horse battery").unwrap().0;
        let (agent, person, other) = (Signed { state: &state, token: &p.claude }, Signed { state: &state, token: &p.ada }, Signed { state: &state, token: &dan });
        let text = "import { action, approval, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"payouts\")\n\nexport const pay = action(async (order) => {\n  const decision = await approval(\"payout\", { expires: \"1h\", people: 2 })\n  state.decided = decision\n  return decision.approved\n})\n";
        let plan = agent::plan(text).unwrap();
        for u in &plan.units {
            json_of(&person, "POST", "/unit", Some(Body::Bytes(u.clone()))).unwrap();
        }
        let preview = json_of(&person, "POST", "/deploy/preview", Some(Body::Json(json!({ "slug": "payouts", "unit": plan.unit })))).unwrap();
        json_of(&person, "POST", "/deploy", Some(Body::Json(json!({ "slug": "payouts", "unit": plan.unit, "acknowledge": preview["needs_approval"], "document": plan.document })))).unwrap();

        // Started by a person (agents run an action only once it lets them).
        let ran = tool_as(&person, "run", json!({ "slug": "payouts", "action": "pay", "input": { "amount": 900 } })).unwrap();
        assert!(ran.starts_with("action pay is waiting: run "), "{ran}");
        let run_id = json_of(&person, "GET", "/deployments/payouts/runs", None).unwrap()["runs"][0]["run_id"].as_str().unwrap().to_string();
        let decide = format!("/deployments/payouts/runs/{run_id}/decide");
        json_of(&other, "POST", &decide, Some(Body::Json(json!({ "name": "payout", "approve": true, "reason": "matches the invoice" })))).unwrap();

        let listed = tool_as(&agent, "runs", json!({ "slug": "payouts" })).unwrap();
        assert!(listed.contains(&format!("run {} (action pay, started by ada, since ", &run_id[..12])), "{listed}");
        assert!(listed.contains("waits at approval \"payout\", 2 people's, 1 of 2: dan (\"matches the invoice\") approved, until "), "{listed}");
        assert!(listed.contains("no tool decides it") && listed.contains("holon decide payouts <run> <name> --approve"), "{listed}");
        assert!(tool_as(&agent, "deployments", json!({})).unwrap().contains("1 waiting (`runs` says for what)"));
        // The agent's own decision is refused before the route: deciding is a person's.
        let (status, body) = agent.send("POST", &decide, Some(Body::Json(json!({ "name": "payout", "approve": true })))).unwrap();
        assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));
        assert!(String::from_utf8_lossy(&body).contains("approving, granting, starting, stopping and deleting are a person's"), "{}", String::from_utf8_lossy(&body));
        let waiting = json_of(&person, "GET", "/deployments/payouts/runs", None).unwrap();
        assert_eq!(waiting["runs"][0]["waiting"][0]["approved"].as_array().unwrap().len(), 1, "still dan's alone: {waiting}");
    }

    fn tool_as(api: &(dyn Api + Sync), name: &str, args: Value) -> Result<String, String> {
        let connect = || -> Result<Box<dyn Api + '_>, String> {
            struct By<'b>(&'b (dyn Api + Sync));
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
        call_tool(name, &args, &connect, &Ctx::default())
    }

    /// Which sites' pages a flow answers (`browsers`) is a person's to widen: a page from a new one
    /// reads what the flow answers, in its visitor's browser. An agent's version that keeps them,
    /// or answers fewer, deploys; one that adds a site waits for a person.
    #[test]
    fn an_agent_doesnt_open_a_flow_to_new_sites() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let (agent, person) = (Signed { state: &state, token: &p.claude }, Signed { state: &state, token: &p.ada });
        let text = "import { flow, route } from \"holon\"\n\nflow(\"sites\", { port: 8232, browsers: \"https://app.example\" })\n\nroute.get(\"/hi\", () => \"hi\")\n";
        let plan = agent::plan(text).unwrap();
        for unit in &plan.units {
            json_of(&person, "POST", "/unit", Some(Body::Bytes(unit.clone()))).unwrap();
        }
        let ask = |browsers: Value, acknowledge: Value| Some(Body::Json(json!({ "slug": "sites", "unit": plan.unit, "acknowledge": acknowledge, "document": plan.document, "browsers": browsers })));
        let preview = json_of(&person, "POST", "/deploy/preview", Some(Body::Json(json!({ "slug": "sites", "unit": plan.unit })))).unwrap();
        json_of(&person, "POST", "/deploy", ask(json!(["https://app.example"]), preview["needs_approval"].clone())).unwrap();
        assert!(deploy(&agent, &text.replace("\"hi\"", "\"hello\""), Some("sites")).unwrap().starts_with("Deployed \"sites\""), "the same sites");
        let wider = deploy(&agent, &text.replace(r#"browsers: "https://app.example""#, r#"browsers: ["https://app.example", "https://other.example"]"#), Some("sites")).unwrap();
        assert!(wider.starts_with("Not deployed yet") && wider.contains("answer web pages from https://other.example"), "{wider}");
        let any = json_of(&agent, "POST", "/deploy", ask(json!(["*"]), json!([]))).unwrap();
        assert!(any["proposal"]["needs"].as_array().unwrap().contains(&json!("Browser(\"*\")")), "proposed: {any}");
        let browsers = |state: &AppState| state.deployments.lock().unwrap().get("sites").map(|d| d.browsers.clone());
        assert_eq!(browsers(&state), Some(vec!["https://app.example".to_string()]), "as the person left it");
        json_of(&agent, "POST", "/deploy", ask(json!([]), json!([]))).unwrap();
        assert_eq!(browsers(&state), Some(vec![]), "answering fewer is the agent's to do");
        assert_eq!(person.send("POST", "/deployments/sites/stop", None).unwrap().0, 200);
    }

    /// An agent's new code never inherits a secret a person let the old code read (redaction
    /// catches only the literal value, so the agent could read it out encoded), and an agent
    /// never starts a deployment a person stopped by deploying over it.
    #[test]
    fn an_agent_neither_inherits_a_secret_nor_starts_a_stopped_deployment() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let (agent, person) = (Signed { state: &state, token: &p.claude }, Signed { state: &state, token: &p.ada });
        let v1 = "import { flow, route, sha256, status } from \"holon\"\nimport api_key from \"secret:api_key\"\n\nflow(\"peek\", { port: 8231 })\n\nroute.get(\"/k\", () => {\n  return { k: sha256(api_key) } ?? status(400)\n})\n";
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
        // Answering with what is computed from the key is a permission of its own.
        assert_eq!(preview["needs_approval"], json!(["SecretAnswer(\"api_key\")", "SecretRead(\"api_key\")"]), "{preview}");
        let proposed = json_of(&agent, "POST", "/deploy", ask(&leaky.unit, json!([]))).unwrap();
        assert!(proposed["proposal"]["needs"].as_array().unwrap().contains(&json!("SecretAnswer(\"api_key\")")), "proposed, not deployed: {proposed}");
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
    /// else saved under that name; over its own earlier proposal, it does.
    #[test]
    fn a_flow_someone_else_saved_is_not_replaced() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let (agent, person) = (Signed { state: &state, token: &p.claude }, Signed { state: &state, token: &p.ada });
        let text = holon_author::samples::sample("hello-api").unwrap().text;
        json_of(&person, "POST", "/flows", Some(Body::Json(json!({ "name": "hello-api", "document": {} })))).unwrap();
        let refused = deploy(&agent, text, None).unwrap_err();
        assert!(refused.contains("saved by ada") && refused.contains("another name"), "{refused}");
        assert!(state.proposals.all().is_empty(), "nor is it proposed");
        assert!(deploy(&agent, text, Some("mine")).unwrap().starts_with("Not deployed yet"));
        assert!(deploy(&agent, text, Some("mine")).unwrap().starts_with("Not deployed yet"), "its own earlier proposal is replaced");
        assert_eq!(state.proposals.all().iter().map(|p| p.state).collect::<Vec<_>>(), [crate::proposals::Decision::Replaced, crate::proposals::Decision::Waiting]);
    }

    /// What needs a person's approval is a proposal: listed for a person with the agent's why and
    /// note, declined with a reason the agent reads back through `deployments`, proposed again and
    /// approved when the person deploys it. A person can turn agents' proposals off for a flow.
    #[test]
    fn an_agents_version_waits_as_a_proposal_a_person_declines_or_approves() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let (agent, person) = (Signed { state: &state, token: &p.claude }, Signed { state: &state, token: &p.ada });
        let slug = Some("proposed");
        let said = super::deploy(&agent, SLUGS_TEXT, slug, Asked { why: Some("slugs for the blog"), note: Some("asked by mira") }).unwrap();
        assert!(said.contains("proposed as version 1 of \"proposed\" (proposal 1), with your why") && said.contains("#approvals.proposed.v1"), "{said}");
        let listed = json_of(&person, "GET", "/proposals", None).unwrap();
        let p = &listed["proposals"][0];
        assert_eq!(
            [&p["state"], &p["why"], &p["note"], &p["version"], &p["by"]],
            [&json!("waiting"), &json!("slugs for the blog"), &json!("asked by mira"), &json!(1), &json!("claude")],
            "{listed}"
        );
        assert!(p.get("document").is_none(), "listed without its document");
        let one = json_of(&person, "GET", "/proposals/1", None).unwrap();
        assert!(one["document"]["source"].as_str().is_some_and(|s| s.contains("flow(\"slugs\"")), "with it, its text: {one}");
        // An agent doesn't decide it; a person declines it, with a reason the agent reads.
        assert_eq!(agent.send("POST", "/proposals/1/decline", Some(Body::Json(json!({ "reason": "mine" })))).unwrap().0, 403);
        json_of(&person, "POST", "/proposals/1/decline", Some(Body::Json(json!({ "reason": "name its table first" })))).unwrap();
        assert_eq!(person.send("POST", "/proposals/1/decline", Some(Body::Json(json!({})))).unwrap().0, 409, "declined once");
        let read = deployments(&agent, Some("proposed")).unwrap();
        assert!(read.contains("proposal 1 was declined by ada at") && read.contains(": name its table first"), "{read}");
        // Proposed again; the person deploys it, which approves it.
        assert!(deploy(&agent, SLUGS_TEXT, slug).unwrap().contains("(proposal 2)"));
        let plan = agent::plan(SLUGS_TEXT).unwrap();
        let preview = json_of(&person, "POST", "/deploy/preview", Some(Body::Json(json!({ "slug": "proposed", "unit": plan.unit })))).unwrap();
        json_of(&person, "POST", "/deploy", Some(Body::Json(json!({ "slug": "proposed", "unit": plan.unit, "acknowledge": preview["needs_approval"], "document": plan.document })))).unwrap();
        let all = json_of(&person, "GET", "/proposals", None).unwrap();
        let states: Vec<(u64, &str)> = all["proposals"].as_array().unwrap().iter().map(|p| (p["id"].as_u64().unwrap(), p["state"].as_str().unwrap())).collect();
        assert_eq!(states, [(2, "approved"), (1, "declined")], "the decided, newest first");
        assert_eq!(all["proposals"][0]["decided_by"], "ada");
        // Off, an agent's deploy of it is refused: neither proposed nor saved. It is on its timeline.
        json_of(&person, "POST", "/deployments/proposed/settings", Some(Body::Json(json!({ "agents_propose": false })))).unwrap();
        assert_eq!(json_of(&person, "GET", "/deployments", None).unwrap()["deployments"][0]["agents_propose"], false);
        let off = deploy(&agent, &SLUGS_TEXT.replace("\"unsigned\"", "\"not signed\""), slug).unwrap_err();
        assert!(off.contains("turned off agents proposing versions of \"proposed\""), "{off}");
        assert_eq!(state.proposals.all().len(), 2);
        let events = json_of(&person, "GET", "/deployments/proposed/events", None).unwrap().to_string();
        assert!(events.contains("agents may no longer propose versions"), "{events}");
        assert_eq!(person.send("POST", "/deployments/proposed/stop", None).unwrap().0, 200);
    }

    /// A host whose policy takes no proposals from agents (holon.toml: agents.propose = false)
    /// turns an agent's version that needs a person away, saving nothing and proposing nothing;
    /// the policy says so; a person deploys it as before. The default policy takes them.
    #[test]
    fn a_host_that_takes_no_proposals_turns_an_agents_version_away() {
        assert!(crate::permissions::Policy::default().agents_propose, "on by default");
        assert!(crate::permissions::Policy::parse("[agents]\npropose = true").unwrap().agents_propose);
        assert!(crate::permissions::Policy::parse("[agents]\nask = true").is_err(), "an unknown key is an error");
        let mut state = test_state();
        *state.policy.write().unwrap() = std::sync::Arc::new(crate::permissions::Policy::parse("[agents]\npropose = false").unwrap());
        let p = with_people(&mut state);
        let (agent, person) = (Signed { state: &state, token: &p.claude }, Signed { state: &state, token: &p.ada });
        let refused = deploy(&agent, SLUGS_TEXT, Some("proposed")).unwrap_err();
        assert!(refused.contains("this host takes no proposals from agents (holon.toml: agents.propose = false)") && refused.contains("A person deploys it"), "{refused}");
        assert!(state.proposals.all().is_empty(), "nothing proposed");
        let saved = json_of(&person, "GET", "/flows", None).unwrap().to_string();
        assert!(!saved.contains("proposed") && !saved.contains("slugs"), "nothing saved: {saved}");
        assert_eq!(json_of(&person, "GET", "/permissions", None).unwrap()["agents_propose"], false);
        let plan = agent::plan(SLUGS_TEXT).unwrap();
        let preview = json_of(&person, "POST", "/deploy/preview", Some(Body::Json(json!({ "slug": "proposed", "unit": plan.unit })))).unwrap();
        json_of(&person, "POST", "/deploy", Some(Body::Json(json!({ "slug": "proposed", "unit": plan.unit, "acknowledge": preview["needs_approval"], "document": plan.document })))).unwrap();
        assert_eq!(person.send("POST", "/deployments/proposed/stop", None).unwrap().0, 200);
    }

    /// A saved flow reads back as the text the agent wrote; what is deployed reads in full,
    /// text and all, before it has ever run.
    #[test]
    fn what_is_saved_and_deployed_reads_back_as_text() {
        let mut state = test_state();
        let p = with_people(&mut state);
        assert!(tool(&state, &p.claude, "deployments", json!({})).unwrap().starts_with("Nothing is deployed"));
        assert!(tool(&state, &p.claude, "flows", json!({})).unwrap().starts_with("No flows are saved"));
        // Saved, not deployed: listed, and its text is the text as written, comments and all.
        let saved = tool(&state, &p.claude, "save", json!({ "text": SLUGS_TEXT })).unwrap();
        assert!(saved.starts_with("Saved \"slugs\"") && saved.contains("receive requests"), "{saved}");
        let listed = tool(&state, &p.claude, "flows", json!({})).unwrap();
        assert!(listed.contains("slugs: saved") && listed.contains("not deployed"), "{listed}");
        let text = tool(&state, &p.claude, "flows", json!({ "name": "slugs" })).unwrap();
        assert!(text.ends_with(SLUGS_TEXT.trim_end()), "{text}");
        assert!(tool(&state, &p.claude, "deployments", json!({})).unwrap().contains("Saved, never deployed"));
        assert!(tool(&state, &p.claude, "deployments", json!({ "slug": "slugs" })).unwrap().contains("isn't deployed. It is saved"));
        assert!(tool(&state, &p.claude, "deployments", json!({ "slug": "nobody" })).unwrap_err().contains("nothing named \"nobody\""));
        assert!(tool(&state, &p.claude, "flows", json!({ "name": "nobody" })).is_err());
        // A person deploys it; then it reads in full, though it has never run.
        let plan = agent::plan(SLUGS_TEXT).unwrap();
        for unit in &plan.units {
            route(&state, &req_as(&p.ada, "POST", "/unit", unit.clone()));
        }
        let preview: Value = serde_json::from_slice(&body_bytes(route(&state, &req_as(&p.ada, "POST", "/deploy/preview", serde_json::to_vec(&json!({ "slug": "slugs", "unit": plan.unit })).unwrap())))).unwrap();
        let approve = json!({ "slug": "slugs", "unit": plan.unit, "acknowledge": preview["needs_approval"], "document": plan.document });
        assert_eq!(route(&state, &req_as(&p.ada, "POST", "/deploy", serde_json::to_vec(&approve).unwrap())).status_code, 200);
        let all = tool(&state, &p.claude, "deployments", json!({})).unwrap();
        assert!(all.contains("slugs: running") && all.contains("0 runs, 0 failed"), "{all}");
        assert!(!all.contains("never deployed"), "{all}");
        let one = tool(&state, &p.claude, "deployments", json!({ "slug": "slugs" })).unwrap();
        assert!(one.contains("Its routes: POST /slugs"), "{one}");
        assert!(one.contains("It may:") && one.contains("receive requests"), "{one}");
        assert!(one.contains("Nothing waits for a person."), "{one}");
        assert!(one.contains("Its text, as deployed:") && one.contains("route.post("), "{one}");
        assert!(tool(&state, &p.claude, "flows", json!({})).unwrap().contains("deployed under that name"));
    }

    /// What a deployment waits on a person for, each said as what they do.
    #[test]
    fn a_deployment_says_what_it_waits_on_a_person_for() {
        let running = json!({ "slug": "s", "status": "running", "deployed_at_epoch_seconds": 100 });
        assert!(waits_for(&running, None).is_empty());
        assert!(waits_for(&running, Some(&json!({ "saved_at_epoch_seconds": 90 }))).is_empty(), "the version saved before is the one deployed");
        let newer = waits_for(&running, Some(&json!({ "saved_at_epoch_seconds": 200, "saved_by": "claude" })));
        assert!(newer[0].contains("saved since by claude") && newer[0].contains("flows page"), "{newer:?}");
        let stuck = json!({ "slug": "s", "status": "paused", "attention": "its grant changed", "listen_error": "port 8200 is taken", "deployed_at_epoch_seconds": 100 });
        let waits = waits_for(&stuck, None);
        assert_eq!(waits.len(), 3, "{waits:?}");
        assert!(waits[0].contains("its grant changed") && waits[1].contains("it is paused") && waits[2].contains("port 8200 is taken"), "{waits:?}");
    }

    /// A call that waits on a person doesn't hold up the connection: a `ping` is answered while it
    /// waits, a request that asked for progress is told every second the call is alive, and the
    /// cancel of a request stops it and leaves it unanswered, as the protocol asks.
    #[test]
    fn a_waiting_call_is_pinged_around_told_of_and_cancelled() {
        use std::sync::mpsc;

        /// What a test says to `serve`, a line at a time, as a client would.
        struct Lines {
            rx: mpsc::Receiver<String>,
            pending: Vec<u8>,
            at: usize,
        }
        impl Read for Lines {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.at >= self.pending.len() {
                    match self.rx.recv() {
                        Ok(line) => {
                            self.pending = format!("{line}\n").into_bytes();
                            self.at = 0;
                        }
                        Err(_) => return Ok(0),
                    }
                }
                let n = buf.len().min(self.pending.len() - self.at);
                buf[..n].copy_from_slice(&self.pending[self.at..self.at + n]);
                self.at += n;
                Ok(n)
            }
        }
        /// What `serve` says, kept where the test can read it as it comes.
        #[derive(Clone, Default)]
        struct Said(Arc<Mutex<Vec<u8>>>);
        impl Write for Said {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        /// A server whose proposal for "tide" nobody decides.
        struct Undecided;
        impl Api for Undecided {
            fn server(&self) -> String {
                "the script".into()
            }
            fn send(&self, _: &str, _: &str, _: Option<Body>) -> Result<(u16, Vec<u8>), String> {
                Ok((200, json!({ "proposals": [{ "id": 1, "slug": "tide", "state": "waiting" }] }).to_string().into_bytes()))
            }
        }
        let connect = || -> Result<Box<dyn Api>, String> { Ok(Box::new(Undecided)) };
        let (tx, rx) = mpsc::channel::<String>();
        let said = Said::default();
        // The lines `serve` has finished: `writeln!` hands a line to the writer in pieces, and the
        // test reads while it writes, so what follows the last newline may be half a message.
        let lines = |said: &Said| -> Vec<Value> {
            let said = said.0.lock().unwrap().clone();
            let whole = said.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            String::from_utf8(said[..whole].to_vec()).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
        };
        let until = |said: &Said, what: &str, found: &dyn Fn(&Value) -> bool| {
            let by = std::time::Instant::now() + Duration::from_secs(6);
            while std::time::Instant::now() < by {
                if lines(said).iter().any(found) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("never saw {what}: {:?}", lines(said));
        };
        std::thread::scope(|scope| {
            let server = scope.spawn(|| serve(std::io::BufReader::new(Lines { rx, pending: Vec::new(), at: 0 }), said.clone(), &connect));
            let send = |v: Value| tx.send(v.to_string()).unwrap();
            send(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } } }));
            send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
            send(json!({ "jsonrpc": "2.0", "id": 10, "method": "tools/call", "params": { "name": "deployments", "arguments": { "slug": "tide", "wait": 30 }, "_meta": { "progressToken": "p1" } } }));
            send(json!({ "jsonrpc": "2.0", "id": 11, "method": "ping" }));
            // The ping is answered while the call waits, which is not answered.
            until(&said, "the ping's answer", &|v| v["id"] == 11);
            assert!(!lines(&said).iter().any(|v| v["id"] == 10), "the waiting call answered already");
            // It says it is alive, with the token it was given, and how far it has got.
            until(&said, "a progress notification", &|v| v["method"] == "notifications/progress" && v["params"]["progressToken"] == "p1" && v["params"]["total"] == 30);
            // Cancelled: it stops, and nothing of it is said afterwards.
            send(json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 10, "reason": "no longer needed" } }));
            std::thread::sleep(Duration::from_millis(1500));
            let before = lines(&said).len();
            std::thread::sleep(Duration::from_millis(1500));
            assert!(!lines(&said).iter().any(|v| v["id"] == 10), "a cancelled call is not answered: {:?}", lines(&said));
            assert_eq!(lines(&said).len(), before, "and it has stopped reporting");
            drop(tx);
            server.join().unwrap().unwrap();
        });
    }

    #[test]
    fn the_protocol_initializes_lists_and_calls_tools_one_line_at_a_time() {
        let mut state = test_state();
        let p = with_people(&mut state);
        let connect = || -> Result<Box<dyn Api + '_>, String> { Ok(Box::new(Signed { state: &state, token: &p.claude })) };
        let input = [
            // Before `initialize` only `ping` is answered.
            r#"{"jsonrpc":"2.0","id":90,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":91,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"reference","arguments":{"topic":"concat"}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"check","arguments":{"text":"import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.get(\"/a\", () => nope)\n"}}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope"}}"#,
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"check","arguments":{"text":5}}}"#,
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"call","arguments":{"slug":"s","method":"GET","path":"/","headers":"x"}}}"#,
            r#"{"jsonrpc":"2.0","id":6,"method":"resources/list"}"#,
            "not json",
            r#"{"jsonrpc":"2.0","id":7,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        ]
        .join("\n");
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, &connect).unwrap();
        let all: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(all.len(), 12, "a notification gets no answer");
        // A tool call answers when it is done, on a thread of its own: each answer is found by its
        // request's id, in the order the requests were sent.
        let at = |id: Value| all.iter().find(|a| a["id"] == id).cloned().unwrap_or_else(|| panic!("no answer to {id}: {all:?}"));
        let answers: Vec<Value> = [json!(90), json!(91), json!(1), json!(2), json!(3), json!(4), json!(5), json!(8), json!(9), json!(6), Value::Null, json!(7)].into_iter().map(at).collect();
        assert_eq!(answers[0]["error"]["code"], -32600, "before initialize: {}", answers[0]);
        assert!(answers[0]["error"]["message"].as_str().unwrap().contains("initialize"));
        assert_eq!(answers[1]["result"], json!({}), "a ping is answered at any time");
        let answers: Vec<Value> = answers[2..].to_vec();
        assert_eq!(answers[0]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(answers[0]["result"]["serverInfo"]["name"], "holon");
        let names: Vec<&str> = answers[1]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["reference", "check", "save", "deploy", "replay", "call", "run", "whoami", "deployments", "flows", "runs", "logs", "state", "explain", "history", "restore"]);
        assert_eq!(answers[2]["result"]["isError"], false);
        assert!(answers[2]["result"]["content"][0]["text"].as_str().unwrap().starts_with("concat("));
        assert_eq!(answers[3]["result"]["isError"], true, "a mistake is a tool's failure, said: {}", answers[3]);
        assert!(answers[3]["result"]["content"][0]["text"].as_str().unwrap().contains("unknown name"));
        assert_eq!(answers[4]["error"]["code"], -32602);
        // An argument of the wrong kind is a tool's failure that says so, not "missing".
        for (k, said) in [(5, "`text` must be a text, not a number"), (6, "`headers` must be an object, not a text")] {
            assert_eq!(answers[k]["result"]["isError"], true, "{}", answers[k]);
            assert_eq!(answers[k]["result"]["content"][0]["text"], said);
        }
        assert_eq!(answers[7]["error"]["code"], -32601);
        assert_eq!(answers[8]["error"]["code"], -32700);
        assert_eq!(answers[9]["result"]["protocolVersion"], VERSIONS[VERSIONS.len() - 1]);
    }
}
