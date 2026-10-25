//! `holon mcp setup` and `holon mcp hosts`: `holon mcp` added to an agent host's settings, so
//! nobody edits another program's config by hand (docs/L4s/mcp-setup-L4.md). Each host keeps its
//! servers in its own file, under its own key, in its own shape; the table below says which, from
//! each host's documentation as of 2026-10. A host it doesn't know is added once, as like one it
//! does (`holon mcp hosts add`). Only the entry changes: the rest of the file stays as it was,
//! comments and all (jsonedit.rs, and toml_edit for TOML).
//!
//! No host's file holds a token. For a server elsewhere, the entry names the server
//! (`holon mcp --server URL`) and holon keeps the token (local.rs's `tokens.json`), so a host
//! that has no way to keep a secret is as safe as one that has, and a new token is one command.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::{jsonedit, local};

/// How a host's file holds a server.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Shape {
    /// JSON, `{ "command", "args" }`: most hosts.
    Plain,
    /// JSON, `{ "type": "stdio", "command", "args" }`.
    Stdio,
    /// JSON, `{ "type": "local", "command", "args", "tools": ["*"] }`: GitHub Copilot CLI.
    Local,
    /// JSON, `{ "type": "local", "command": [program, args…], "enabled": true }`: opencode.
    Opencode,
    /// TOML, a table with `command` and `args`: Codex.
    Toml,
    /// YAML under `extensions:`: Goose. Shown to paste, never written.
    Goose,
}

/// A host: where it keeps its servers, and how.
#[derive(Clone, Debug)]
pub(crate) struct Host {
    pub(crate) name: String,
    pub(crate) title: String,
    /// Its own settings file for this user, on this machine; none where it has none.
    pub(crate) file: Option<PathBuf>,
    /// Its project settings file, from the project's folder.
    pub(crate) project: Option<&'static str>,
    /// Where its servers are in the file.
    pub(crate) key: Vec<String>,
    pub(crate) shape: Shape,
    /// Whether it is on this machine: this, or its file, is there.
    found_if: Option<PathBuf>,
    /// Whether its file is written; else the entry is shown, with `then` saying where it goes.
    pub(crate) writes: bool,
    /// What to do after, so it reads the entry (or, when it isn't written, how to add it).
    pub(crate) then: String,
    /// Added with `holon mcp hosts add`.
    pub(crate) custom: bool,
}

impl Host {
    fn found(&self) -> bool {
        self.file.as_deref().is_some_and(Path::exists) || self.found_if.as_deref().is_some_and(Path::exists)
    }
}

/// Where hosts keep their settings on this machine.
pub(crate) struct Places {
    pub(crate) home: PathBuf,
    /// Holon's own folder (`~/.holon`, or `HOLON_HOME`), where the custom hosts are kept.
    pub(crate) holon: PathBuf,
    /// Applications' settings: `%APPDATA%` on Windows, `~/Library/Application Support` on macOS,
    /// `$XDG_CONFIG_HOME` (or `~/.config`) elsewhere.
    pub(crate) app: PathBuf,
    /// `$XDG_CONFIG_HOME`, or `~/.config`, which some hosts use on every system.
    pub(crate) xdg: PathBuf,
    pub(crate) windows: bool,
    /// The environment variables that move a host's folder (`CODEX_HOME`, …), when set.
    pub(crate) vars: BTreeMap<&'static str, PathBuf>,
}

/// The variables that move a host's settings.
const MOVED_BY: &[&str] = &["CODEX_HOME", "COPILOT_HOME", "KIRO_HOME", "CLINE_MCP_SETTINGS_PATH"];

impl Places {
    pub(crate) fn here() -> Result<Places, String> {
        let home = local::home().ok_or("no home folder (USERPROFILE or HOME) to find hosts' settings in")?;
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let xdg = var("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let app = if cfg!(windows) {
            var("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming"))
        } else if cfg!(target_os = "macos") {
            home.join("Library").join("Application Support")
        } else {
            xdg.clone()
        };
        let vars = MOVED_BY.iter().filter_map(|k| var(k).map(|v| (*k, v))).collect();
        let holon = local::holon_home().unwrap_or_else(|| home.join(".holon"));
        Ok(Places { home, holon, app, xdg, windows: cfg!(windows), vars })
    }
}

/// `rel` (with `/`s) under `base`, in this system's own separators.
fn under(base: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(base.to_path_buf(), |path, part| path.join(part))
}

/// Every host holon knows, with where each keeps its settings here.
pub(crate) fn builtin(p: &Places) -> Vec<Host> {
    let home = |rel: &str| under(&p.home, rel);
    let moved = |var: &str, rel: &str| p.vars.get(var).cloned().unwrap_or_else(|| home(rel));
    // Some keep theirs in `~/.config` on macOS and Linux, and in `%APPDATA%` on Windows.
    let config = |rel: &str| under(if p.windows { &p.app } else { &p.xdg }, rel);
    let code_user = p.app.join("Code").join("User");
    let host = |name: &str, title: &str, file: Option<PathBuf>, project: Option<&'static str>, key: &str, shape: Shape, found_if: Option<PathBuf>, then: &str| Host {
        name: name.into(),
        title: title.into(),
        file,
        project,
        key: key.split('.').map(str::to_string).collect(),
        shape,
        found_if,
        writes: shape != Shape::Goose,
        then: then.into(),
        custom: false,
    };
    use Shape::*;
    vec![
        host("claude-code", "Claude Code", Some(home(".claude.json")), Some(".mcp.json"), "mcpServers", Stdio, Some(home(".claude")), "start a new session to load it"),
        host("claude-desktop", "Claude Desktop", Some(p.app.join("Claude").join("claude_desktop_config.json")), None, "mcpServers", Plain, Some(p.app.join("Claude")), "quit Claude and open it again"),
        host("cursor", "Cursor", Some(home(".cursor/mcp.json")), Some(".cursor/mcp.json"), "mcpServers", Stdio, Some(home(".cursor")), "check it under Cursor Settings → MCP"),
        host("vscode", "VS Code (Copilot)", Some(code_user.join("mcp.json")), Some(".vscode/mcp.json"), "servers", Stdio, Some(code_user.clone()), "start it from the MCP servers in the Extensions view"),
        host(
            "visual-studio",
            "Visual Studio",
            p.windows.then(|| home(".mcp.json")),
            Some(".vs/mcp.json"),
            "servers",
            Stdio,
            None,
            "find it in Copilot Chat's tools (the + button) in a new chat",
        ),
        host("copilot-cli", "GitHub Copilot CLI", Some(moved("COPILOT_HOME", ".copilot").join("mcp-config.json")), Some(".github/mcp.json"), "mcpServers", Local, Some(moved("COPILOT_HOME", ".copilot")), "start a new copilot session"),
        host("devin", "Devin Desktop and Devin CLI", Some(config("devin/mcp_config.json")), Some(".devin/mcp_config.json"), "mcpServers", Plain, Some(config("devin")), "refresh the MCP servers in Devin's settings"),
        host("windsurf", "Windsurf (before Devin Desktop)", Some(home(".codeium/windsurf/mcp_config.json")), None, "mcpServers", Plain, Some(home(".codeium/windsurf")), "refresh Cascade's MCP servers"),
        host(
            "zed",
            "Zed",
            Some(if p.windows { p.app.join("Zed").join("settings.json") } else { p.xdg.join("zed").join("settings.json") }),
            None,
            "context_servers",
            Plain,
            Some(if p.windows { p.app.join("Zed") } else { p.xdg.join("zed") }),
            "check it in the Agent panel's settings (Zed loads it at once)",
        ),
        host("gemini", "Gemini CLI", Some(home(".gemini/settings.json")), Some(".gemini/settings.json"), "mcpServers", Plain, Some(home(".gemini")), "start gemini again"),
        host("antigravity", "Antigravity", Some(home(".gemini/config/mcp_config.json")), Some(".agents/mcp_config.json"), "mcpServers", Plain, Some(home(".gemini/config")), "refresh it in the MCP manager (/mcp)"),
        host("codex", "OpenAI Codex", Some(moved("CODEX_HOME", ".codex").join("config.toml")), Some(".codex/config.toml"), "mcp_servers", Toml, Some(moved("CODEX_HOME", ".codex")), "start codex again"),
        host(
            "cline",
            "Cline CLI",
            Some(p.vars.get("CLINE_MCP_SETTINGS_PATH").cloned().unwrap_or_else(|| home(".cline/data/settings/cline_mcp_settings.json"))),
            None,
            "mcpServers",
            Plain,
            Some(home(".cline")),
            "start cline again",
        ),
        host(
            "cline-vscode",
            "Cline in VS Code",
            Some(under(&code_user, "globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json")),
            None,
            "mcpServers",
            Plain,
            Some(under(&code_user, "globalStorage/saoudrizwan.claude-dev")),
            "check it under Cline's MCP Servers button",
        ),
        host("junie", "Junie", Some(home(".junie/mcp/mcp.json")), Some(".junie/mcp/mcp.json"), "mcpServers", Plain, Some(home(".junie")), "start junie again"),
        host("kiro", "Kiro and Kiro CLI", Some(moved("KIRO_HOME", ".kiro").join("settings").join("mcp.json")), Some(".kiro/settings/mcp.json"), "mcpServers", Plain, Some(moved("KIRO_HOME", ".kiro")), "check it in Kiro's MCP servers"),
        host("opencode", "opencode", Some(p.xdg.join("opencode").join("opencode.json")), Some("opencode.json"), "mcp", Opencode, Some(p.xdg.join("opencode")), "start opencode again"),
        host("lmstudio", "LM Studio", Some(home(".lmstudio/mcp.json")), None, "mcpServers", Plain, Some(home(".lmstudio")), "restart LM Studio if it isn't listed"),
        host("continue", "Continue", None, Some(".continue/mcpServers/holon.json"), "mcpServers", Plain, None, "open the project in Continue"),
        host(
            "goose",
            "Goose",
            Some(if p.windows { p.app.join("Block").join("goose").join("config").join("config.yaml") } else { p.xdg.join("goose").join("config.yaml") }),
            None,
            "extensions",
            Goose,
            Some(if p.windows { p.app.join("Block").join("goose") } else { p.xdg.join("goose") }),
            "add it under `extensions:` in its config.yaml, or with goose configure → Add Extension → Command-line Extension",
        ),
        Host {
            writes: false,
            ..host(
                "jetbrains",
                "JetBrains AI Assistant",
                None,
                None,
                "mcpServers",
                Plain,
                None,
                "in the IDE: Settings → Tools → AI Assistant → Model Context Protocol (MCP) → Add, as JSON, and paste it",
            )
        },
    ]
}

// ── custom hosts ────────────────────────────────────────────────────────────────────────────

/// Where custom hosts are kept: `~/.holon/mcp-hosts.toml`.
fn custom_file(p: &Places) -> PathBuf {
    p.holon.join("mcp-hosts.toml")
}

/// The custom hosts in `file`, each as like the host it names.
fn custom_hosts(file: &Path, known: &[Host]) -> Result<Vec<Host>, String> {
    let Ok(text) = std::fs::read_to_string(file) else { return Ok(Vec::new()) };
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{}: {e}", file.display()))?;
    let mut hosts = Vec::new();
    let Some(table) = doc.get("hosts").and_then(|h| h.as_table_like()) else { return Ok(hosts) };
    for (name, item) in table.iter() {
        let field = |k: &str| item.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let (Some(config), Some(like)) = (field("config"), field("like")) else {
            return Err(format!("{}: [hosts.{name}] needs config and like", file.display()));
        };
        let base = known.iter().find(|h| h.name == like && h.writes).ok_or_else(|| format!("{}: [hosts.{name}] is like {like:?}, which isn't a host whose file holon writes", file.display()))?;
        hosts.push(Host {
            name: name.to_string(),
            title: field("title").unwrap_or_else(|| name.to_string()),
            file: Some(PathBuf::from(config)),
            project: None,
            key: field("key").map(|k| k.split('.').map(str::to_string).collect()).unwrap_or_else(|| base.key.clone()),
            shape: base.shape,
            found_if: None,
            writes: true,
            then: format!("restart {name} if it doesn't load it"),
            custom: true,
        });
    }
    Ok(hosts)
}

/// Every host: the built-in ones, then this user's own.
fn all_hosts(p: &Places) -> Result<Vec<Host>, String> {
    let mut hosts = builtin(p);
    let custom = custom_hosts(&custom_file(p), &hosts)?;
    hosts.extend(custom);
    Ok(hosts)
}

// ── the entry ───────────────────────────────────────────────────────────────────────────────

/// The entry, for a JSON host.
fn json_entry(shape: Shape, program: &str, args: &[String]) -> Value {
    match shape {
        Shape::Stdio => json!({ "type": "stdio", "command": program, "args": args }),
        Shape::Local => json!({ "type": "local", "command": program, "args": args, "tools": ["*"] }),
        Shape::Opencode => {
            let command: Vec<&str> = std::iter::once(program).chain(args.iter().map(String::as_str)).collect();
            json!({ "type": "local", "command": command, "enabled": true })
        }
        Shape::Plain | Shape::Toml | Shape::Goose => json!({ "command": program, "args": args }),
    }
}

/// An entry already there, as the command line it runs.
fn command_line(entry: &Value) -> String {
    let words: Vec<String> = match entry.get("command") {
        Some(Value::Array(a)) => a.iter().map(|v| v.as_str().unwrap_or("?").to_string()).collect(),
        Some(Value::String(c)) => std::iter::once(c.clone()).chain(entry.get("args").and_then(Value::as_array).into_iter().flatten().map(|v| v.as_str().unwrap_or("?").to_string())).collect(),
        _ => entry.get("cmd").and_then(Value::as_str).map(|c| vec![c.to_string()]).unwrap_or_else(|| vec!["?".into()]),
    };
    words.iter().map(|w| if w.contains(' ') { format!("\"{w}\"") } else { w.clone() }).collect::<Vec<_>>().join(" ")
}

/// What setting it up in one file did.
#[derive(Debug, PartialEq)]
enum Done {
    Added,
    /// It was there, running something else (that command line).
    Replaced(String),
    Unchanged,
    Removed,
    NotThere,
}

/// The entry `name` in a host's file text, as JSON.
fn read_entry(shape: Shape, text: &str, key: &[String], name: &str) -> Result<Option<Value>, String> {
    if shape == Shape::Toml {
        let doc: toml_edit::DocumentMut = text.parse::<toml_edit::DocumentMut>().map_err(|e| e.to_string())?;
        let mut item = doc.as_item();
        for k in key.iter().map(String::as_str).chain([name]) {
            match item.get(k) {
                Some(i) => item = i,
                None => return Ok(None),
            }
        }
        let command = item.get("command").and_then(|v| v.as_str()).map(str::to_string);
        let args: Vec<String> = item.get("args").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
        return Ok(Some(json!({ "command": command, "args": args })));
    }
    let path: Vec<&str> = key.iter().map(String::as_str).chain([name]).collect();
    jsonedit::get(text, &path)
}

/// `text` with the entry set (merged into one that is there: what else it holds is kept), or
/// with `None`, removed; and what that did.
fn edit(shape: Shape, text: &str, key: &[String], name: &str, entry: Option<&Value>) -> Result<(String, Done), String> {
    let before = read_entry(shape, text, key, name)?;
    let Some(entry) = entry else {
        if before.is_none() {
            return Ok((text.to_string(), Done::NotThere));
        }
        let out = if shape == Shape::Toml {
            let mut doc: toml_edit::DocumentMut = text.parse::<toml_edit::DocumentMut>().map_err(|e| e.to_string())?;
            let mut table = doc.as_table_mut();
            for k in key {
                table = table.get_mut(k).and_then(|i| i.as_table_mut()).ok_or("not a table")?;
            }
            table.remove(name);
            doc.to_string()
        } else {
            let path: Vec<&str> = key.iter().map(String::as_str).chain([name]).collect();
            jsonedit::remove(text, &path)?.unwrap_or_else(|| text.to_string())
        };
        return Ok((out, Done::Removed));
    };
    let done = match &before {
        None => Done::Added,
        Some(b) if command_line(b) == command_line(entry) => Done::Unchanged,
        Some(b) => Done::Replaced(command_line(b)),
    };
    if shape == Shape::Toml {
        let mut doc: toml_edit::DocumentMut = text.parse::<toml_edit::DocumentMut>().map_err(|e| e.to_string())?;
        let mut table = doc.as_table_mut();
        for k in key {
            let item = table.entry(k).or_insert_with(|| {
                let mut t = toml_edit::Table::new();
                t.set_implicit(true);
                toml_edit::Item::Table(t)
            });
            table = item.as_table_mut().ok_or_else(|| format!("{k} isn't a table"))?;
        }
        let item = table.entry(name).or_insert_with(toml_edit::table);
        let t = item.as_table_mut().ok_or_else(|| format!("{name} isn't a table"))?;
        t["command"] = toml_edit::value(entry["command"].as_str().unwrap_or_default());
        let args: toml_edit::Array = entry["args"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        t["args"] = toml_edit::value(args);
        return Ok((doc.to_string(), done));
    }
    let merged = match before {
        Some(Value::Object(mut was)) => {
            for (k, v) in entry.as_object().into_iter().flatten() {
                was.insert(k.clone(), v.clone());
            }
            Value::Object(was)
        }
        _ => entry.clone(),
    };
    let path: Vec<&str> = key.iter().map(String::as_str).chain([name]).collect();
    let out = if done == Done::Unchanged { text.to_string() } else { jsonedit::set(text, &path, &merged)? };
    Ok((out, done))
}

/// The entry as a person pastes it: the host's key around it.
fn snippet(host: &Host, name: &str, program: &str, args: &[String]) -> String {
    match host.shape {
        Shape::Toml => edit(Shape::Toml, "", &host.key, name, Some(&json_entry(Shape::Toml, program, args))).map(|(text, _)| text).unwrap_or_default(),
        Shape::Goose => {
            let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
            format!(
                "extensions:\n  {name}:\n    type: stdio\n    name: {name}\n    enabled: true\n    cmd: {}\n    args: [{}]\n    timeout: 300\n",
                quote(program),
                args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(", ")
            )
        }
        shape => {
            let path: Vec<&str> = host.key.iter().map(String::as_str).chain([name]).collect();
            jsonedit::set("", &path, &json_entry(shape, program, args)).unwrap_or_default()
        }
    }
}

/// Write `text` to `path` whole or not at all, keeping the file's permissions.
fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".holon-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let written = std::fs::write(&tmp, text).and_then(|()| {
        if let Ok(m) = std::fs::metadata(path) {
            std::fs::set_permissions(&tmp, m.permissions())?;
        }
        std::fs::rename(&tmp, path)
    });
    written.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("couldn't write {}: {e}", path.display())
    })
}

/// Set up (or remove) the entry in one host's file: a sentence saying what it did.
fn apply(host: &Host, file: &Path, name: &str, entry: Option<&Value>) -> Result<String, String> {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("couldn't read {}: {e}", file.display())),
    };
    let (out, done) = edit(host.shape, &text, &host.key, name, entry).map_err(|e| format!("{} isn't one holon can edit ({e}); it is left as it was", file.display()))?;
    if out != text {
        write_file(file, &out)?;
    }
    let place = format!("{} ({})", host.title, file.display());
    Ok(match done {
        Done::Added => format!("{place}: added {name}. Next, {}.", host.then),
        Done::Replaced(was) => format!("{place}: {name} ran `{was}`; now it runs this one. Next, {}.", host.then),
        Done::Unchanged => format!("{place}: {name} is already there."),
        Done::Removed => format!("{place}: removed {name}."),
        Done::NotThere => format!("{place}: no {name} to remove."),
    })
}

// ── the token, for a server elsewhere ───────────────────────────────────────────────────────

/// Whether `url` (as `local::server_url` writes it) is on this machine.
fn on_this_machine(url: &str) -> bool {
    let host = url.split("://").nth(1).unwrap_or(url);
    let host = host.split('/').next().unwrap_or("");
    let host = if let Some(rest) = host.strip_prefix('[') { rest.split(']').next().unwrap_or("") } else { host.rsplit_once(':').map_or(host, |(h, _)| h) };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// A line from the terminal, not shown as it is typed.
fn read_hidden(prompt: &str) -> Result<String, String> {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let stdin = std::io::stdin();
    let terminal = stdin.is_terminal();
    let restore = if terminal { hide_input() } else { None };
    let mut line = String::new();
    let read = stdin.read_line(&mut line);
    if let Some(restore) = restore {
        restore();
    }
    // The Enter wasn't shown, or there was none (it was piped).
    eprintln!();
    read.map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}

/// Stop the terminal showing what is typed; what puts it back.
#[cfg(windows)]
fn hide_input() -> Option<Box<dyn FnOnce()>> {
    use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_ECHO_INPUT, STD_INPUT_HANDLE};
    // SAFETY: the standard input handle and a mode word, as the console API documents.
    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(handle, &mut mode) == 0 || SetConsoleMode(handle, mode & !ENABLE_ECHO_INPUT) == 0 {
            return None;
        }
        let handle = handle as usize;
        Some(Box::new(move || {
            SetConsoleMode(handle as _, mode);
        }))
    }
}

#[cfg(not(windows))]
fn hide_input() -> Option<Box<dyn FnOnce()>> {
    let stty = |arg: &str| std::process::Command::new("stty").arg(arg).stdin(std::process::Stdio::inherit()).status().is_ok_and(|s| s.success());
    stty("-echo").then(|| {
        Box::new(move || {
            stty("echo");
        }) as Box<dyn FnOnce()>
    })
}

/// Who `token` signs in as on `server`, and with what role; `None` if it can't be asked.
fn who(server: &str, token: &str) -> Result<Option<(String, String)>, String> {
    match ureq::get(&format!("{server}/auth/me")).timeout(Duration::from_secs(10)).set("Authorization", &format!("Bearer {token}")).call() {
        Ok(r) => {
            let me: Value = r.into_string().ok().and_then(|b| serde_json::from_str(&b).ok()).unwrap_or(Value::Null);
            let field = |k: &str| me.get(k).and_then(Value::as_str).unwrap_or("?").to_string();
            Ok(Some((field("name"), field("role"))))
        }
        Err(ureq::Error::Status(401, _)) => Err(format!("{server} doesn't take this token (401): make one with role agent on its admin page")),
        Err(ureq::Error::Status(code, _)) => Err(format!("{server} answered {code} to the token")),
        Err(_) => Ok(None),
    }
}

/// The token for a server elsewhere: `HOLON_TOKEN`, or asked for; checked with it, and kept.
fn keep_token_for(server: &str) -> Result<(), String> {
    let file = local::tokens_file().ok_or("no home folder to keep the token in")?;
    let kept = local::saved_token(&file, server);
    let given = std::env::var("HOLON_TOKEN").ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let token = match given {
        Some(t) => t,
        None => {
            let ask = if kept.is_some() {
                format!("An API token for {server} (Enter keeps the one holon has): ")
            } else {
                format!("An API token for {server}, with role agent (its admin page makes one): ")
            };
            read_hidden(&ask)?
        }
    };
    let token = match (token.is_empty(), kept) {
        (false, _) => token,
        (true, Some(k)) => k,
        (true, None) => return Err("no token: one with role agent, from the server's admin page, is how holon mcp signs in there".into()),
    };
    match who(server, &token)? {
        Some((name, role)) if role == "agent" => println!("The token signs in to {server} as {name}, an agent."),
        Some((name, role)) => println!(
            "The token signs in to {server} as {name}, with role {role}. holon mcp still asks as an agent, but only a token with role agent keeps an agent with a shell from approving: make one on the admin page."
        ),
        None => println!("{server} didn't answer, so the token isn't checked yet."),
    }
    local::keep_token(&file, server, Some(&token))?;
    println!("Kept in {}, readable only by you; no host's file holds it.", file.display());
    Ok(())
}

// ── the commands ────────────────────────────────────────────────────────────────────────────

pub(crate) const SETUP_USAGE: &str = "usage: holon mcp setup [HOST…|--all] [options]

Add holon mcp to an agent host's settings: its entry in the host's own file, the rest of the file
left as it was. With no host, the hosts holon knows and which are on this machine.

  HOST…                  the hosts to set it up in (holon mcp hosts lists them)
  --all                  every host found on this machine
  --server PORT|URL      reach that server (a server elsewhere: holon asks for an API token
                         with role agent, checks it, and keeps it; HOLON_TOKEN gives it)
  --project DIR          the hosts' project files in DIR instead of yours (the entry runs
                         `holon`, found on PATH, since the file is shared)
  --name NAME            the entry's name (default holon), e.g. for a second server
  --print                show the entry and where it goes; write nothing
  --remove               take the entry out (with --server, and forget its token)
  --config PATH --like HOST [--key KEY]
                         a file holon doesn't know, written as HOST's is (holon mcp hosts
                         add keeps one for next time)";

pub(crate) const HOSTS_USAGE: &str = "usage: holon mcp hosts [add NAME --config PATH --like HOST [--key KEY] [--title TITLE] | remove NAME]

The hosts holon mcp setup knows: where each keeps its settings on this machine, whether it is
here, and the holon entry in it. `add` keeps a host of your own (in ~/.holon/mcp-hosts.toml),
written as the host it is like; `--key` is where its servers are, dotted (mcp.servers).";

struct Options {
    hosts: Vec<String>,
    all: bool,
    server: Option<String>,
    project: Option<PathBuf>,
    name: String,
    print: bool,
    remove: bool,
    config: Option<PathBuf>,
    like: Option<String>,
    key: Option<String>,
}

fn options(args: &[String]) -> Result<Options, String> {
    let mut o = Options { hosts: vec![], all: false, server: None, project: None, name: "holon".into(), print: false, remove: false, config: None, like: None, key: None };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |what: &str| it.next().cloned().ok_or_else(|| format!("{a} needs {what}"));
        match a.as_str() {
            "--all" => o.all = true,
            "--print" => o.print = true,
            "--remove" => o.remove = true,
            "--server" => o.server = Some(value("a port or URL")?),
            "--project" => o.project = Some(PathBuf::from(value("a folder")?)),
            "--name" => o.name = value("a name")?,
            "--config" => o.config = Some(PathBuf::from(value("a file")?)),
            "--like" => o.like = Some(value("a host's name")?),
            "--key" => o.key = Some(value("a key, dotted")?),
            s if s.starts_with('-') => return Err(format!("{s}?")),
            s => o.hosts.push(s.to_string()),
        }
    }
    if o.name.trim().is_empty() {
        return Err("--name needs a name".into());
    }
    if o.config.is_some() != o.like.is_some() {
        return Err("--config and --like go together: the file, and the host it is written as".into());
    }
    Ok(o)
}

/// `holon mcp setup`.
pub(crate) fn setup(args: &[String]) -> i32 {
    if args.iter().any(|a| matches!(a.as_str(), "-h" | "--help" | "help")) {
        println!("{SETUP_USAGE}");
        return 0;
    }
    let o = match options(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("holon mcp setup: {e}\n\n{SETUP_USAGE}");
            return 2;
        }
    };
    match setup_with(o) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("holon mcp setup: {e}");
            1
        }
    }
}

fn setup_with(o: Options) -> Result<i32, String> {
    let places = Places::here()?;
    let known = all_hosts(&places)?;
    let mut chosen: Vec<Host> = Vec::new();
    if let (Some(config), Some(like)) = (&o.config, &o.like) {
        let base = known.iter().find(|h| h.name == *like && h.writes).ok_or_else(|| format!("--like {like}: not a host whose file holon writes (holon mcp hosts lists them)"))?;
        chosen.push(Host {
            name: "custom".into(),
            title: config.display().to_string(),
            file: Some(config.clone()),
            project: None,
            key: o.key.as_deref().map(|k| k.split('.').map(str::to_string).collect()).unwrap_or_else(|| base.key.clone()),
            shape: base.shape,
            found_if: None,
            writes: true,
            then: "restart the host if it doesn't load it".into(),
            custom: true,
        });
    }
    for name in &o.hosts {
        let host = known.iter().find(|h| h.name == *name).ok_or_else(|| format!("no host {name:?}; holon mcp hosts lists the ones it knows, and adds one"))?;
        chosen.push(host.clone());
    }
    if o.all {
        chosen.extend(known.iter().filter(|h| h.writes && h.found() && !o.hosts.contains(&h.name)).cloned());
    }
    if chosen.is_empty() {
        print!("{}", table(&known, &o.name));
        println!("\nName the hosts to set it up in (holon mcp setup claude-code cursor), or --all for every one found here.");
        return Ok(if o.all { 1 } else { 2 });
    }

    let server = o.server.as_deref().map(local::server_url);
    let program = if o.project.is_some() {
        "holon".to_string()
    } else {
        std::env::current_exe().map_err(|e| format!("where is this program? {e}"))?.display().to_string()
    };
    let mut mcp_args = vec!["mcp".to_string()];
    if let Some(s) = &server {
        mcp_args.extend(["--server".to_string(), s.clone()]);
    }

    if o.print {
        for host in &chosen {
            let file = match &o.project {
                Some(dir) => host.project.map(|p| under(dir, p)),
                None => host.file.clone(),
            };
            let place = file.map(|f| f.display().to_string()).unwrap_or_else(|| "its settings".into());
            println!("{} ({place}):\n{}", host.title, snippet(host, &o.name, &program, &mcp_args).trim_end());
            if !host.writes {
                println!("{}.", host.then);
            }
            println!();
        }
        return Ok(0);
    }

    if let Some(s) = server.as_deref().filter(|s| !on_this_machine(s)) {
        if o.remove {
            if let Some(file) = local::tokens_file()
                && local::keep_token(&file, s, None)?
            {
                println!("Forgot the token for {s}.");
            }
        } else {
            keep_token_for(s)?;
        }
    }

    let mut failed = false;
    for host in &chosen {
        if !host.writes {
            println!("{} keeps its servers where holon doesn't write; {}:\n{}", host.title, host.then, snippet(host, &o.name, &program, &mcp_args).trim_end());
            continue;
        }
        let file = match &o.project {
            Some(dir) => match host.project {
                Some(p) => under(dir, p),
                None => {
                    eprintln!("{} has no project settings file.", host.title);
                    failed = true;
                    continue;
                }
            },
            None => match &host.file {
                Some(f) => f.clone(),
                None => {
                    eprintln!("{} has no settings file of yours on this system{}.", host.title, if host.project.is_some() { " (--project DIR sets it up in a project)" } else { "" });
                    failed = true;
                    continue;
                }
            },
        };
        let entry = (!o.remove).then(|| json_entry(host.shape, &program, &mcp_args));
        match apply(host, &file, &o.name, entry.as_ref()) {
            Ok(said) => println!("{said}"),
            Err(e) => {
                eprintln!("{}: {e}", host.title);
                failed = true;
            }
        }
    }
    Ok(if failed { 1 } else { 0 })
}

/// The hosts, one line each: name, title, where its settings are, and what is there.
fn table(hosts: &[Host], name: &str) -> String {
    let rows: Vec<[String; 4]> = hosts
        .iter()
        .map(|h| {
            let place = h.file.as_ref().map(|f| f.display().to_string()).unwrap_or_else(|| if h.project.is_some() { "(project files only)".into() } else { "(its settings dialog)".into() });
            let state = match &h.file {
                _ if !h.writes && h.file.is_none() => "shown to paste".to_string(),
                Some(f) if f.exists() => match std::fs::read_to_string(f).map_err(|e| e.to_string()).and_then(|t| read_entry(h.shape, &t, &h.key, name)) {
                    Ok(Some(e)) => format!("{name}: {}", command_line(&e)),
                    Ok(None) if h.writes => "found".into(),
                    Ok(None) => "found; shown to paste".into(),
                    Err(e) => format!("can't read it: {e}"),
                },
                _ if h.found() => "found".into(),
                _ => "not found".into(),
            };
            let title = if h.custom { format!("{} (yours)", h.title) } else { h.title.clone() };
            [h.name.clone(), title, place, state]
        })
        .collect();
    let width = |i: usize| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0);
    let (w0, w1) = (width(0), width(1));
    let mut out = String::new();
    for r in &rows {
        out.push_str(&format!("  {:w0$}  {:w1$}  {}\n      {}\n", r[0], r[1], r[3], r[2]));
    }
    out
}

/// `holon mcp hosts`.
pub(crate) fn hosts(args: &[String]) -> i32 {
    if args.iter().any(|a| matches!(a.as_str(), "-h" | "--help" | "help")) {
        println!("{HOSTS_USAGE}");
        return 0;
    }
    match hosts_with(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("holon mcp hosts: {e}");
            2
        }
    }
}

fn hosts_with(args: &[String]) -> Result<(), String> {
    let places = Places::here()?;
    let file = custom_file(&places);
    match args.first().map(String::as_str) {
        None => {
            let hosts = all_hosts(&places)?;
            println!("The hosts holon mcp setup knows, whether each is on this machine (or the holon entry it has), and where it keeps its settings:\n");
            print!("{}", table(&hosts, "holon"));
            println!("\nholon mcp setup HOST adds it; holon mcp hosts add NAME --config PATH --like HOST adds a host.");
            Ok(())
        }
        Some("add") => {
            let name = args.get(1).filter(|n| !n.starts_with('-')).ok_or("add needs the host's name")?;
            let (mut config, mut like, mut key, mut title) = (None, None, None, None);
            let mut it = args[2..].iter();
            while let Some(a) = it.next() {
                let v = it.next().cloned().ok_or_else(|| format!("{a} needs a value"))?;
                match a.as_str() {
                    "--config" => config = Some(v),
                    "--like" => like = Some(v),
                    "--key" => key = Some(v),
                    "--title" => title = Some(v),
                    other => return Err(format!("{other}?\n\n{HOSTS_USAGE}")),
                }
            }
            let (config, like) = (config.ok_or("add needs --config PATH")?, like.ok_or("add needs --like HOST")?);
            let config = absolute(&config).display().to_string();
            let text = add_custom(&std::fs::read_to_string(&file).unwrap_or_default(), &builtin(&places), name, &config, &like, key.as_deref(), title.as_deref())?;
            write_file(&file, &text)?;
            println!("Added {name} ({config}), written as {like}'s file is: holon mcp setup {name} sets it up.");
            Ok(())
        }
        Some("remove") => {
            let name = args.get(1).ok_or("remove needs the host's name")?;
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{}: {e}", file.display()))?;
            let gone = doc.get_mut("hosts").and_then(|h| h.as_table_like_mut()).and_then(|t| t.remove(name)).is_some();
            if !gone {
                return Err(format!("no host of yours named {name:?}"));
            }
            write_file(&file, &doc.to_string())?;
            println!("Removed {name}. Its file is as it was: holon mcp setup {name} --remove, before this, takes the entry out.");
            Ok(())
        }
        Some(other) => Err(format!("{other}?\n\n{HOSTS_USAGE}")),
    }
}

fn absolute(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() { p } else { std::env::current_dir().map(|d| d.join(&p)).unwrap_or(p) }
}

/// The custom hosts file's text with `name` added (or replaced).
fn add_custom(text: &str, builtin: &[Host], name: &str, config: &str, like: &str, key: Option<&str>, title: Option<&str>) -> Result<String, String> {
    if builtin.iter().any(|h| h.name == name) {
        return Err(format!("{name} is a host holon knows already"));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("{name:?}: a host's name is letters, digits, - and _"));
    }
    if !builtin.iter().any(|h| h.name == like && h.writes) {
        return Err(format!("--like {like}: not a host whose file holon writes"));
    }
    let mut doc: toml_edit::DocumentMut = text.parse::<toml_edit::DocumentMut>().map_err(|e| e.to_string())?;
    let hosts = doc.entry("hosts").or_insert_with(|| {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        toml_edit::Item::Table(t)
    });
    let hosts = hosts.as_table_mut().ok_or("hosts isn't a table")?;
    let mut t = toml_edit::Table::new();
    if let Some(title) = title {
        t["title"] = toml_edit::value(title);
    }
    t["config"] = toml_edit::value(config);
    t["like"] = toml_edit::value(like);
    if let Some(key) = key {
        t["key"] = toml_edit::value(key);
    }
    hosts.insert(name, toml_edit::Item::Table(t));
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(windows: bool) -> Places {
        let home = PathBuf::from(if windows { r"C:\Users\ada" } else { "/home/ada" });
        let app = if windows { home.join(r"AppData\Roaming") } else { home.join(".config") };
        Places { xdg: home.join(".config"), holon: home.join(".holon"), home, app, windows, vars: BTreeMap::new() }
    }

    fn host(name: &str) -> Host {
        builtin(&places(false)).into_iter().find(|h| h.name == name).unwrap()
    }

    const PROGRAM: &str = "/home/ada/.holon/bin/holon";

    fn args() -> Vec<String> {
        vec!["mcp".into()]
    }

    #[test]
    fn every_host_has_a_name_a_place_and_a_shape() {
        for windows in [false, true] {
            let hosts = builtin(&places(windows));
            let mut names: Vec<&str> = hosts.iter().map(|h| h.name.as_str()).collect();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), hosts.len(), "names are unique");
            for h in &hosts {
                assert!(h.file.is_some() || h.project.is_some() || !h.writes, "{} has somewhere to go", h.name);
                assert!(!h.key.is_empty() && !h.then.is_empty(), "{}", h.name);
            }
        }
        assert_eq!(host("zed").file.unwrap(), PathBuf::from("/home/ada/.config/zed/settings.json"));
        let windows = builtin(&places(true));
        let zed = windows.iter().find(|h| h.name == "zed").unwrap();
        assert_eq!(zed.file.as_ref().unwrap(), &places(true).app.join("Zed").join("settings.json"));
        assert!(host("visual-studio").file.is_none(), "Visual Studio is Windows only");
        let mut moved = places(false);
        moved.vars.insert("CODEX_HOME", PathBuf::from("/opt/codex"));
        assert_eq!(builtin(&moved).iter().find(|h| h.name == "codex").unwrap().file.as_ref().unwrap(), &PathBuf::from("/opt/codex/config.toml"));
    }

    /// Each host's entry, as its documentation shows one.
    #[test]
    fn each_host_gets_the_entry_it_reads() {
        let entry = |name: &str| {
            let h = host(name);
            let (text, done) = edit(h.shape, "", &h.key, "holon", Some(&json_entry(h.shape, PROGRAM, &args()))).unwrap();
            assert_eq!(done, Done::Added);
            text
        };
        assert_eq!(entry("claude-code"), "{\n  \"mcpServers\": {\n    \"holon\": {\n      \"type\": \"stdio\",\n      \"command\": \"/home/ada/.holon/bin/holon\",\n      \"args\": [\n        \"mcp\"\n      ]\n    }\n  }\n}\n");
        assert!(entry("claude-desktop").contains("\"mcpServers\": {\n    \"holon\": {\n      \"command\""));
        assert!(entry("vscode").starts_with("{\n  \"servers\": {\n    \"holon\": {\n      \"type\": \"stdio\""));
        assert!(entry("zed").starts_with("{\n  \"context_servers\": {\n    \"holon\": {\n      \"command\""));
        assert!(entry("copilot-cli").contains("\"type\": \"local\"") && entry("copilot-cli").contains("\"tools\": [\n        \"*\"\n      ]"));
        let opencode: Value = serde_json::from_str(&entry("opencode")).unwrap();
        assert_eq!(opencode, json!({ "mcp": { "holon": { "type": "local", "command": [PROGRAM, "mcp"], "enabled": true } } }));
        assert_eq!(entry("codex"), "[mcp_servers.holon]\ncommand = \"/home/ada/.holon/bin/holon\"\nargs = [\"mcp\"]\n");
        let goose = snippet(&host("goose"), "holon", PROGRAM, &["mcp".into(), "--server".into(), "https://h.example".into()]);
        assert!(goose.contains("    cmd: '/home/ada/.holon/bin/holon'\n    args: ['mcp', '--server', 'https://h.example']\n"), "{goose}");
    }

    #[test]
    fn set_up_twice_is_once_and_another_command_is_replaced_keeping_the_rest() {
        let h = host("cursor");
        let theirs = "{\n  \"mcpServers\": {\n    \"other\": { \"command\": \"x\" },\n    \"holon\": { \"command\": \"holon-old\", \"args\": [\"mcp\"], \"env\": { \"A\": \"1\" } }\n  }\n}\n";
        let entry = json_entry(h.shape, PROGRAM, &args());
        let (once, done) = edit(h.shape, theirs, &h.key, "holon", Some(&entry)).unwrap();
        assert_eq!(done, Done::Replaced("holon-old mcp".into()));
        assert!(once.contains("\"other\": { \"command\": \"x\" },"), "{once}");
        assert_eq!(jsonedit::get(&once, &["mcpServers", "holon", "env", "A"]).unwrap(), Some(json!("1")), "what else it held is kept");
        let (twice, done) = edit(h.shape, &once, &h.key, "holon", Some(&entry)).unwrap();
        assert_eq!((twice.as_str(), done), (once.as_str(), Done::Unchanged));
        let (removed, done) = edit(h.shape, &once, &h.key, "holon", None).unwrap();
        assert_eq!(done, Done::Removed);
        assert_eq!(removed, "{\n  \"mcpServers\": {\n    \"other\": { \"command\": \"x\" }\n  }\n}\n");
        assert_eq!(edit(h.shape, &removed, &h.key, "holon", None).unwrap().1, Done::NotThere);
    }

    #[test]
    fn codex_keeps_its_comments_and_other_servers() {
        let h = host("codex");
        let theirs = "# mine\nmodel = \"o5\"\n\n[mcp_servers.docs]\ncommand = \"docs\"\n";
        let (out, done) = edit(h.shape, theirs, &h.key, "holon", Some(&json_entry(h.shape, PROGRAM, &args()))).unwrap();
        assert_eq!(done, Done::Added);
        assert_eq!(out, "# mine\nmodel = \"o5\"\n\n[mcp_servers.docs]\ncommand = \"docs\"\n\n[mcp_servers.holon]\ncommand = \"/home/ada/.holon/bin/holon\"\nargs = [\"mcp\"]\n");
        let (back, _) = edit(h.shape, &out, &h.key, "holon", None).unwrap();
        assert_eq!(back.trim_end(), theirs.trim_end());
    }

    #[test]
    fn a_custom_host_is_written_as_the_host_it_is_like() {
        let known = builtin(&places(false));
        let text = add_custom("", &known, "my-ide", "/home/ada/.my-ide/mcp.json", "vscode", Some("mcp.servers"), Some("My IDE")).unwrap();
        assert_eq!(text, "[hosts.my-ide]\ntitle = \"My IDE\"\nconfig = \"/home/ada/.my-ide/mcp.json\"\nlike = \"vscode\"\nkey = \"mcp.servers\"\n");
        let dir = std::env::temp_dir().join(format!("holon-mcphosts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("mcp-hosts.toml");
        std::fs::write(&file, &text).unwrap();
        let custom = custom_hosts(&file, &known).unwrap();
        assert_eq!(custom.len(), 1);
        assert_eq!((custom[0].title.as_str(), custom[0].shape, custom[0].key.clone()), ("My IDE", Shape::Stdio, vec!["mcp".to_string(), "servers".to_string()]));
        assert!(add_custom("", &known, "cursor", "/x", "vscode", None, None).unwrap_err().contains("knows already"));
        assert!(add_custom("", &known, "x", "/x", "jetbrains", None, None).unwrap_err().contains("not a host whose file holon writes"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_server_here_or_elsewhere() {
        assert!(on_this_machine("http://127.0.0.1:7420"));
        assert!(on_this_machine("http://localhost:7420"));
        assert!(on_this_machine("http://[::1]:7420"));
        assert!(!on_this_machine("https://holon.example.com"));
        assert!(!on_this_machine("http://10.0.0.5:7420"));
    }

    #[test]
    fn a_file_is_written_whole_and_set_up_in_it_says_what_it_did() {
        let dir = std::env::temp_dir().join(format!("holon-apply-{}", std::process::id()));
        let file = dir.join("deep").join("mcp.json");
        let h = host("cursor");
        let entry = json_entry(h.shape, PROGRAM, &args());
        let said = apply(&h, &file, "holon", Some(&entry)).unwrap();
        assert!(said.starts_with("Cursor (") && said.ends_with(": added holon. Next, check it under Cursor Settings → MCP."), "{said}");
        assert!(apply(&h, &file, "holon", Some(&entry)).unwrap().contains("already there"));
        std::fs::write(&file, "{ not json").unwrap();
        let refused = apply(&h, &file, "holon", Some(&entry)).unwrap_err();
        assert!(refused.contains("left as it was"), "{refused}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(dir);
    }
}
