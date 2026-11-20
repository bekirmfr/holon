//! `holon deploy <file>`: compile a flow and deploy it to a running server, as the editor does.
//! The server and sign-in come from the one this user runs here (`local.rs`), or `--server` and
//! a person's `holon login` or `HOLON_TOKEN`. Useful from CI or a test harness that wants to
//! deploy without a browser.

use std::io::Read;
use std::time::Duration;

use holon_author::agent;
use serde_json::{json, Value};

use crate::local;
use crate::mcp::{Api, Body};

const USAGE: &str = "usage: holon deploy <file> [options]

  Compiles a flow and deploys it to a running server, as the editor's Deploy does. When the flow
  needs what it isn't granted yet, deploy prints what that is and exits 1: approve it in the
  editor by deploying from there, or here with --approve. Signed in as an agent (its token in
  HOLON_TOKEN), it approves nothing: each version is proposed for a person, who approves it in
  the editor's Approvals (holon wait <slug> ends when they decide). So is one that needs what
  isn't granted, signed in as someone who may propose versions but not approve them.

  --manifest               what it touches and may do, as a deploy would show it; nothing is deployed
  --json                   with --manifest: as JSON, with the text it would deploy and the text
                           deployed now (null when nothing is), for a diff before approving
  --slug NAME              deploy under NAME instead of the flow's own name
  --approve                approve, as you, what it asks for that isn't granted yet, and say
                           what that was
  --note TEXT              kept with the grant, on its deploy in the flow's history: why it
                           was approved, what was checked
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon deploy shop.holon.ts
  holon deploy shop.holon.ts --approve --note \"checked the new host with Ada\"
  holon deploy shop.holon.ts --slug shop-staging --server https://holon.example.com

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  as you with holon login (and with none running here, the one you signed in to needs no
  --server), or as an agent or a service with its token in HOLON_TOKEN.";

pub(crate) fn run(args: &[String]) -> i32 {
    let mut file: Option<String> = None;
    let mut server_arg: Option<String> = None;
    let mut slug_arg: Option<String> = None;
    let mut approve = false;
    let mut manifest = false;
    let mut as_json = false;
    let mut note: Option<String> = None;
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server_arg = args.next().cloned(),
            "--slug" => slug_arg = args.next().cloned(),
            "--approve" => approve = true,
            "--note" => note = args.next().cloned(),
            "--manifest" => manifest = true,
            "--json" => as_json = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return 0;
            }
            _ if file.is_none() && !a.starts_with('-') => file = Some(a.clone()),
            _ => {
                eprintln!("holon deploy: no option {a:?}\n\n{USAGE}");
                return 2;
            }
        }
    }
    let Some(file) = file else {
        eprintln!("{USAGE}");
        return 2;
    };
    let source = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("{file}: no such file");
            return 1;
        }
        Err(e) => {
            eprintln!("{file}: {e}");
            return 1;
        }
    };

    let plan = match agent::plan(&source) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("holon deploy: {file}: {e}");
            return 1;
        }
    };

    let slug = slug_arg.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&plan.name).to_string();

    let target = match local::target(server_arg.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("holon deploy: {e}");
            return 1;
        }
    };
    let server = target.server.clone();
    let api = Person(target);

    let mut staged = match stage(&api, &plan, &slug) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("holon deploy: {e}");
            return 1;
        }
    };
    let needs = staged.needs.clone();

    if manifest && as_json {
        // The text it would deploy, beside the text deployed now (null when nothing is deployed as `slug`):
        // what a person reads before approving a deploy that changes the code.
        let deployed = match crate::mcp::json_of(&api, "GET", &format!("/deployments/{}/document", crate::mcp::seg(&slug)), None) {
            Ok(doc) => Value::String(agent::text_of(&slug, &doc).unwrap_or_default()),
            Err(e) if e.contains("answered 404") => Value::Null,
            Err(e) => {
                eprintln!("holon deploy: {e}");
                return 1;
            }
        };
        let new_text = match agent::text_of(&slug, &plan.document) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("holon deploy: {e}");
                return 1;
            }
        };
        let starts: Vec<String> = if plan.routes.is_empty() { plan.document["trace"].as_object().map(|t| t.keys().cloned().collect()).unwrap_or_default() } else { plan.routes.clone() };
        let asks: Vec<String> = needs.iter().map(|cap| agent::describe_sending(cap, &plan.sent)).collect();
        let page = json!({
            "slug": slug,
            "touches": crate::touches::text(&crate::touches::lines(&staged.touches, &staged.settings)),
            "starts": starts,
            "needs_approval": needs,
            "asks": asks,
            "granted_now": staged.granted_now,
            "items": staged.items,
            "withheld": staged.withheld,
            "deployed_text": deployed,
            "new_text": new_text,
        });
        crate::cmdline::print_json(&page);
        return 0;
    }
    if manifest {
        println!("{slug:?} on {server}: its manifest (nothing is deployed)");
        println!("What it touches (+ new, - no longer, ~ changed, ! not ready on this server):");
        for line in crate::touches::text(&crate::touches::lines(&staged.touches, &staged.settings)) {
            println!("{line}");
        }
        println!("What starts it:");
        let handlers: Vec<&String> = plan.document["trace"].as_object().map(|t| t.keys().collect()).unwrap_or_default();
        for r in if plan.routes.is_empty() { handlers } else { plan.routes.iter().collect() } {
            println!("  {r}");
        }
        println!("It may:");
        for cap in &needs {
            println!("  - {}", agent::describe_sending(cap, &plan.sent));
        }
        return 0;
    }

    // Whoever doesn't approve what it asks for (an agent, or one who may propose but not
    // approve) sends it as it is: the server keeps it as a proposal for a person, and --approve
    // isn't theirs to give.
    let approves = staged.approves;
    if approve && !approves {
        eprintln!("holon deploy: you don't approve what \"{slug}\" asks for here, so --approve gives nothing: it is proposed for a person who does");
    }
    if !needs.is_empty() && !approve && approves {
        eprintln!("holon deploy: \"{slug}\" needs approval before it can deploy.");
        eprintln!("What it touches (+ new, - no longer, ~ changed, ! not ready on this server):");
        for line in crate::touches::text(&crate::touches::lines(&staged.touches, &staged.settings)) {
            eprintln!("{line}");
        }
        eprintln!("It asks to:");
        for cap in &needs {
            eprintln!("  - {}", agent::describe_sending(cap, &plan.sent));
        }
        eprintln!("\nApprove it here with holon deploy --approve, or in the editor at {server}: open \"{slug}\" from the flows page and deploy it there.");
        return 1;
    }

    if let Some(note) = note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        staged.body["note"] = json!(note);
    }
    match commit(&api, &plan, staged, plan.document.clone()) {
        Err(e) => {
            eprintln!("holon deploy: {e}");
            return 1;
        }
        // Kept for a person to deploy: whoever signed in doesn't approve it, or what it needs is
        // beyond them.
        Ok(Some(proposal)) => {
            for line in proposed_lines(&slug, &server, &plan, &proposal) {
                println!("{line}");
            }
            return 0;
        }
        Ok(None) => {}
    }

    println!("deployed {slug:?} ({}) on {server}", &plan.unit[..12]);
    if approves && !needs.is_empty() {
        println!("  approved, as you:");
        for cap in &needs {
            println!("    - {}", agent::describe_sending(cap, &plan.sent));
        }
    }
    // What starts its runs: its routes and its websockets' blocks (`WebSocket /room/:name open`),
    // else its other handlers (`every 10s`, `action reset`).
    let handlers: Vec<&String> = plan.document["trace"].as_object().map(|t| t.keys().collect()).unwrap_or_default();
    let mut sockets: Vec<&String> = handlers.iter().copied().filter(|h| h.starts_with("WebSocket ")).collect();
    // Each websocket's blocks in the order they run: open, message, close.
    sockets.sort_by_key(|h| h.rsplit_once(' ').map(|(path, on)| (path.to_string(), ["open", "message", "close"].iter().position(|o| *o == on))));
    for r in if plan.routes.is_empty() { handlers.clone() } else { plan.routes.iter().chain(sockets).collect() } {
        println!("  {r}");
    }
    0
}

/// What `holon deploy` says of a version kept as `proposal` (proposals.rs): which version and
/// proposal, what it asks for, where a person decides it, and how to hear what they decide.
fn proposed_lines(slug: &str, server: &str, plan: &agent::Plan, proposal: &Value) -> Vec<String> {
    let number = |k: &str| proposal.get(k).and_then(Value::as_u64).unwrap_or(0);
    let mut out = vec![format!("proposed {slug:?} ({}) on {server}, as version {} (proposal {}): it waits for a person to approve it", &plan.unit[..12], number("version"), number("id"))];
    let needs: Vec<&str> = proposal["needs"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if needs.is_empty() {
        out.push("  it asks for nothing new".into());
    } else {
        out.push("  it asks to:".into());
        out.extend(needs.iter().map(|cap| format!("    - {}", agent::describe_sending(cap, &plan.sent))));
    }
    out.push("  they approve it in the editor's Approvals, which deploys it, or decline it with a reason".into());
    out.push(format!("  holon wait {slug} --server {server} ends when they decide (exit 0 approved, 1 declined, with the reason)"));
    out
}

/// A deploy staged on a server: its units sent, and what the server says deploying it needs.
pub(crate) struct Staged {
    /// The deploy request, without its acknowledgements and document yet.
    body: Value,
    /// What it asks for that isn't granted yet: a person approves these by deploying it.
    pub(crate) needs: Vec<String>,
    /// What it touches (its secrets not set yet among them) and the settings that change, as the
    /// preview answered them (touches.rs).
    pub(crate) touches: Value,
    pub(crate) settings: Value,
    /// The permissions the deployment is granted now (none for a first deploy), and what its version
    /// asks for, less what is withheld: beside each other, what a deploy changes in its grant.
    pub(crate) granted_now: Option<Vec<String>>,
    pub(crate) items: Vec<String>,
    pub(crate) withheld: Vec<String>,
    /// Whether whoever signed in approves what it asks for: not an agent, nor one who may propose
    /// but not approve, whose deploy acknowledges nothing (proposals.rs).
    pub(crate) approves: bool,
}

impl Staged {
    /// A note kept with the deploy, on its event and in the activity log (deploy.rs): more than its
    /// why for whoever reads what changed. Empty takes it away.
    pub(crate) fn with_note(&mut self, note: &str) {
        match note.trim() {
            "" => _ = self.body.as_object_mut().map(|o| o.remove("note")),
            n => self.body["note"] = serde_json::json!(n),
        }
    }

    /// The note it is sent with, if any.
    pub(crate) fn note(&self) -> Option<&str> {
        self.body["note"].as_str()
    }
}

/// Send `plan`'s units to `server`, and ask what deploying it as `slug` needs a person to approve;
/// or why the server won't run it. `holon deploy` and `holon editor` deploy through this and
/// [`commit`], as the editor does through the same two requests.
pub(crate) fn stage(api: &dyn Api, plan: &agent::Plan, slug: &str) -> Result<Staged, String> {
    // Stage every unit the root needs, children first.
    for unit in &plan.units {
        post_bytes(api, "/unit", unit).map_err(|e| format!("staging a unit: {e}"))?;
    }
    let mut body = json!({ "slug": slug, "unit": plan.unit });
    if let Some(r) = &plan.requirements {
        body["requirements"] = r.clone();
    }
    if let Some(b) = &plan.browsers {
        body["browsers"] = json!(b.split_whitespace().collect::<Vec<_>>());
    }
    let preview = post_json(api, "/deploy/preview", &body)?;
    if let Some(why) = preview.get("refused").and_then(Value::as_str) {
        return Err(format!("this server won't run \"{slug}\": {why}"));
    }
    let list = |k: &str| -> Vec<String> {
        preview.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
    };
    Ok(Staged {
        body,
        needs: list("needs_approval"),
        touches: preview["touches"].clone(),
        settings: preview["settings"].clone(),
        granted_now: preview.get("granted_now").and_then(Value::as_array).map(|_| list("granted_now")),
        items: list("items"),
        withheld: list("withheld"),
        approves: preview.get("approves").and_then(Value::as_bool).unwrap_or(true),
    })
}

/// Deploy what [`stage`] staged, with `document` (the plan's, with its text kept or not):
/// approving what it needs, as whoever `token` signs in as, if they approve it. The proposal it
/// was kept as instead, when they don't (an agent's version, or one who may propose but not
/// approve), or
/// what it needs is beyond them (proposals.rs): their deploy acknowledges nothing, which the
/// server would refuse.
pub(crate) fn commit(api: &dyn Api, plan: &agent::Plan, staged: Staged, document: Value) -> Result<Option<Value>, String> {
    let Staged { mut body, needs, approves, .. } = staged;
    let mut acknowledge = Vec::new();
    if approves {
        acknowledge = plan.may.clone();
        acknowledge.extend(needs.iter().filter(|n| !plan.may.contains(n)).cloned());
    }
    body["acknowledge"] = json!(acknowledge);
    body["document"] = document;
    post_json(api, "/deploy", &body).map(|answer| answer.get("proposal").cloned())
}

/// A server reached as a person: whoever the token signs in as, not as an agent (mcp.rs), so a
/// deploy can approve what it asks for.
pub(crate) struct Person(pub(crate) local::Target);

impl Api for Person {
    fn server(&self) -> String {
        self.0.server.clone()
    }
    fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Vec<u8>), String> {
        let (server, token) = (&self.0.server, self.0.token.as_deref());
        match body {
            None => send(server, token, method, path, None, ""),
            Some(Body::Json(v)) => send(server, token, method, path, Some(v.to_string().as_bytes()), "application/json"),
            Some(Body::Bytes(b)) => send(server, token, method, path, Some(&b), "application/octet-stream"),
        }
    }
    fn sign_in_hint(&self) -> String {
        local::sign_in_hint(&self.0.server, self.0.from)
    }
}

/// Send a request to `api`; the JSON it answered, or an error sentence.
pub(crate) fn call(api: &dyn Api, method: &str, path: &str, body: Option<Body>) -> Result<Value, String> {
    let server = api.server();
    let (status, bytes) = api.send(method, path, body)?;
    if (200..300).contains(&status) {
        return crate::wire::answer(&bytes).map_err(|e| format!("{server} answered, but {e}"));
    }
    // An error's own words, when it says them as JSON; else its text as it is.
    let value = crate::wire::answer(&bytes).unwrap_or(Value::Null);
    let why = value.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| String::from_utf8_lossy(&bytes).trim().to_string());
    Err(match status {
        401 => format!("{server} wants a sign-in ({why}): {}", api.sign_in_hint()),
        403 => format!("{server} refused: {why}"),
        _ => format!("{server} answered {status}: {why}"),
    })
}

/// POST JSON to `api`'s `path`; the response body, or an error sentence.
fn post_json(api: &dyn Api, path: &str, body: &Value) -> Result<Value, String> {
    call(api, "POST", path, Some(Body::Json(body.clone())))
}

/// POST raw bytes to `api`'s `path`.
fn post_bytes(api: &dyn Api, path: &str, body: &[u8]) -> Result<(), String> {
    let server = api.server();
    let (status, bytes) = api.send("POST", path, Some(Body::Bytes(body.to_vec())))?;
    if (200..300).contains(&status) {
        return Ok(());
    }
    let why = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| String::from_utf8_lossy(&bytes).trim().to_string());
    Err(format!("{server} answered {status}: {why}"))
}

fn send(server: &str, token: Option<&str>, method: &str, path: &str, body: Option<&[u8]>, content_type: &str) -> Result<(u16, Vec<u8>), String> {
    let url = format!("{server}{path}");
    let mut req = ureq::request(method, &url).timeout(Duration::from_secs(60));
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let result = match body {
        Some(b) => req.set("Content-Type", content_type).send_bytes(b),
        None => req.call(),
    };
    let response = match result {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => {
            return Err(format!(
                "can't reach {server}: {}\nIs holon serve running there? --server takes the port or address it printed",
                crate::outbound::said(&e)
            ))
        }
    };
    let status = response.status();
    let mut bytes = Vec::new();
    response.into_reader().take(4 << 20).read_to_end(&mut bytes).map_err(|e| format!("reading {server}'s answer: {e}"))?;
    Ok((status, bytes))
}
