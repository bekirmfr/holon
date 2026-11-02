//! `holon deploy <file>`: compile a flow and deploy it to a running server, as the editor does.
//! The server and sign-in come from the one this user runs here (`local.rs`), or `--server` and
//! `HOLON_TOKEN`. Useful from CI or a test harness that wants to deploy without a browser.

use std::io::Read;
use std::time::Duration;

use holon_author::agent;
use serde_json::{json, Value};

use crate::local;
use crate::mcp::{Api, Body};

const USAGE: &str = "usage: holon deploy <file> [options]

  --server PORT|URL  reach this server (default: the one running on this machine)
  --slug NAME        deploy under this name instead of the flow's own name
  --approve          approve what it asks for that isn't granted yet, as you, and say what

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  with an API token in HOLON_TOKEN.

  If the flow needs permissions not yet granted, deploy prints what it needs and exits 1:
  approve them in the editor by deploying from there, or here with --approve, as you.";

pub(crate) fn run(args: &[String]) -> i32 {
    let mut file: Option<String> = None;
    let mut server_arg: Option<String> = None;
    let mut slug_arg: Option<String> = None;
    let mut approve = false;
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server_arg = args.next().cloned(),
            "--slug" => slug_arg = args.next().cloned(),
            "--approve" => approve = true,
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
    let api = Person { server: target.server, token: target.token };

    let staged = match stage(&api, &plan, &slug) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("holon deploy: {e}");
            return 1;
        }
    };
    let needs = staged.needs.clone();

    if !needs.is_empty() && !approve {
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

    if let Err(e) = commit(&api, &plan, staged, plan.document.clone()) {
        eprintln!("holon deploy: {e}");
        return 1;
    }

    println!("deployed {slug:?} ({}) on {server}", &plan.unit[..12]);
    if !needs.is_empty() {
        println!("  approved, as you:");
        for cap in &needs {
            println!("    - {}", agent::describe_sending(cap, &plan.sent));
        }
    }
    if !plan.routes.is_empty() {
        for r in &plan.routes {
            println!("  {r}");
        }
    } else if let Some(i) = plan.interval_seconds {
        println!("  runs every {i}s");
    }
    0
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
    if let Some(i) = plan.interval_seconds {
        body["interval_seconds"] = json!(i);
    }
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
    Ok(Staged { body, needs: list("needs_approval"), touches:preview["touches"].clone(), settings: preview["settings"].clone() })
}

/// Deploy what [`stage`] staged, with `document` (the plan's, with its text kept or not):
/// approving what it needs, as whoever `token` signs in as.
pub(crate) fn commit(api: &dyn Api, plan: &agent::Plan, staged: Staged, document: Value) -> Result<(), String> {
    let Staged { mut body, needs, .. } = staged;
    let mut acknowledge = plan.may.clone();
    acknowledge.extend(needs.iter().filter(|n| !plan.may.contains(n)).cloned());
    body["acknowledge"] = json!(acknowledge);
    body["document"] = document;
    post_json(api, "/deploy", &body).map(|_| ())
}

/// A server reached as a person: whoever the token signs in as, not as an agent (mcp.rs), so a
/// deploy can approve what it asks for.
pub(crate) struct Person {
    pub(crate) server: String,
    pub(crate) token: Option<String>,
}

impl Api for Person {
    fn server(&self) -> String {
        self.server.clone()
    }
    fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Vec<u8>), String> {
        let token = self.token.as_deref();
        match body {
            None => send(&self.server, token, method, path, None, ""),
            Some(Body::Json(v)) => send(&self.server, token, method, path, Some(v.to_string().as_bytes()), "application/json"),
            Some(Body::Bytes(b)) => send(&self.server, token, method, path, Some(&b), "application/octet-stream"),
        }
    }
}

/// Send a request to `api`; the JSON it answered, or an error sentence.
pub(crate) fn call(api: &dyn Api, method: &str, path: &str, body: Option<Body>) -> Result<Value, String> {
    let server = api.server();
    let (status, bytes) = api.send(method, path, body)?;
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok(value);
    }
    let why = value.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| String::from_utf8_lossy(&bytes).trim().to_string());
    Err(match status {
        401 => format!("{server} wants a sign-in: set HOLON_TOKEN to an API token (the editor's admin page → API tokens): {why}"),
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
                "can't reach {server}: {e}\nIs holon serve running there? --server takes the port or address it printed"
            ))
        }
    };
    let status = response.status();
    let mut bytes = Vec::new();
    response.into_reader().take(4 << 20).read_to_end(&mut bytes).map_err(|e| format!("reading {server}'s answer: {e}"))?;
    Ok((status, bytes))
}
