//! What the commands that talk to a running server share: their options (`--server`, the rest),
//! and asking the server for JSON, signed in as `local::target` says.

use serde_json::Value;

use crate::local;

/// A command's arguments: the words, the options given values, and the flags.
pub(crate) struct Args {
    pub(crate) words: Vec<String>,
    values: Vec<(String, String)>,
    flags: Vec<String>,
}

impl Args {
    /// Read `args`: `valued` options take the next argument (`--server 7420`, repeatable), `flags`
    /// none. `--help` is the error `""`; anything else unknown, a sentence saying so.
    pub(crate) fn parse(args: &[String], valued: &[&str], flags: &[&str]) -> Result<Args, String> {
        let mut parsed = Args { words: Vec::new(), values: Vec::new(), flags: Vec::new() };
        let mut args = args.iter();
        while let Some(a) = args.next() {
            if a == "--help" || a == "-h" {
                return Err(String::new());
            }
            if valued.contains(&a.as_str()) || a == "--server" {
                let Some(v) = args.next() else {
                    return Err(format!("{a} takes a value"));
                };
                parsed.values.push((a.clone(), v.clone()));
            } else if flags.contains(&a.as_str()) {
                parsed.flags.push(a.clone());
            } else if a.starts_with("--") {
                return Err(format!("no option {a:?}"));
            } else {
                parsed.words.push(a.clone());
            }
        }
        Ok(parsed)
    }

    pub(crate) fn value(&self, name: &str) -> Option<&str> {
        self.values.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    pub(crate) fn all(&self, name: &str) -> Vec<&str> {
        self.values.iter().filter(|(n, _)| n == name).map(|(_, v)| v.as_str()).collect()
    }

    pub(crate) fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name)
    }

    pub(crate) fn server(&self) -> Option<&str> {
        self.value("--server")
    }

    pub(crate) fn word(&self, i: usize) -> Option<&str> {
        self.words.get(i).map(String::as_str)
    }
}

/// What the commands' usage ends with: where the server is, and how to sign in.
pub(crate) const SIGN_IN: &str = "  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  as you with holon login (and with none running here, the one you signed in to needs no
  --server), or as an agent or a service with its token in HOLON_TOKEN.";

/// Parse `args` for a command named `who` with `usage`, or the exit code: 0 for `--help`
/// (printed), 2 for a mistake (said, with the usage).
pub(crate) fn parse(who: &str, usage: &str, args: &[String], valued: &[&str], flags: &[&str]) -> Result<Args, i32> {
    Args::parse(args, valued, flags).map_err(|why| {
        if why.is_empty() {
            println!("{usage}");
            0
        } else {
            eprintln!("{who}: {why}\n\n{usage}");
            2
        }
    })
}

/// Ask the server; its JSON answer (`null` for an empty one), or, said, the exit code.
pub(crate) fn json(who: &str, args: &Args, method: &str, path: &str, body: Option<&Value>) -> Result<Value, i32> {
    // Every POST names its content type, or the server refuses it: an empty one sends `{}`.
    let empty = Value::Object(Default::default());
    let body = body.or((method == "POST").then_some(&empty));
    let (server, answer) = local::admin_call(who, args.server(), method, path, body)?;
    crate::wire::answer(answer.as_bytes()).map_err(|e| {
        eprintln!("{who}: {server} answered, but {e}");
        1
    })
}

/// Print `value` as JSON, for `--json`.
pub(crate) fn print_json(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
}

/// A path segment, escaped.
pub(crate) fn seg(s: &str) -> String {
    crate::mcp::seg(s)
}

/// Seconds since 1970, as the editor shows a moment.
pub(crate) fn when(epoch_seconds: Option<u64>) -> String {
    epoch_seconds.map(|s| crate::engines::time_text(s as i64 * 1_000_000)).unwrap_or_else(|| "-".into())
}

/// What `--NAME TEXT` or `--NAME-file PATH` gave, or the text piped in with `--NAME -`: never a
/// terminal's, which would be typed in the clear.
pub(crate) fn text_from(who: &str, args: &Args, name: &str) -> Result<Option<String>, i32> {
    if let Some(path) = args.value(&format!("{name}-file")) {
        return std::fs::read_to_string(path).map(Some).map_err(|e| {
            eprintln!("{who}: {path}: {e}");
            1
        });
    }
    match args.value(name) {
        Some("-") => stdin_text(who).map(Some),
        Some(text) => Ok(Some(text.to_string())),
        None => Ok(None),
    }
}

/// Everything piped in on stdin; refused from a terminal, where it would show as it is typed.
pub(crate) fn stdin_text(who: &str) -> Result<String, i32> {
    use std::io::{IsTerminal, Read};
    if std::io::stdin().is_terminal() {
        eprintln!("{who}: pipe it in (… | {who} …): typed here, it would show as you type it");
        return Err(2);
    }
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).map_err(|e| {
        eprintln!("{who}: reading stdin: {e}");
        1
    })?;
    Ok(text)
}

/// A line from the terminal, not shown as it is typed (a password, a token); or the next line
/// piped in.
pub(crate) fn read_hidden(prompt: &str) -> Result<String, String> {
    eprint!("{prompt}");
    let _ = std::io::Write::flush(&mut std::io::stderr());
    let stdin = std::io::stdin();
    let terminal = std::io::IsTerminal::is_terminal(&stdin);
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

/// A line from the terminal, shown as it is typed (a name); or the next line piped in.
pub(crate) fn read_line(prompt: &str) -> Result<String, String> {
    eprint!("{prompt}");
    let _ = std::io::Write::flush(&mut std::io::stderr());
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}

/// Rows as columns, each as wide as its widest cell.
pub(crate) fn table(rows: &[Vec<String>]) -> Vec<String> {
    let widths: Vec<usize> = (0..rows.iter().map(Vec::len).max().unwrap_or(0)).map(|i| rows.iter().filter_map(|r| r.get(i)).map(|c| c.chars().count()).max().unwrap_or(0)).collect();
    rows.iter()
        .map(|r| {
            let cells: Vec<String> = r.iter().enumerate().map(|(i, c)| if i + 1 == r.len() { c.clone() } else { format!("{c:<w$}", w = widths[i]) }).collect();
            cells.join("  ").trim_end().to_string()
        })
        .collect()
}

/// What `holon mail`, `holon feed` and `holon queue` read from their words and flags: the
/// server asked, the deployment a binding is for, the words, and the binding's fields as the
/// server takes them. The terminal editor's Mail, Feeds and Queues parts read a dialog's line
/// the same way (`split_words` first).
#[derive(Default, Debug)]
pub(crate) struct BindArgs {
    pub(crate) server: Option<String>,
    pub(crate) for_deployment: Option<String>,
    pub(crate) words: Vec<String>,
    pub(crate) asked: serde_json::Map<String, Value>,
    pub(crate) help: bool,
}

/// A line as a shell splits it into words: on spaces, but not inside "double" or 'single' quotes,
/// which are taken off ("Treasury <treasury@acme.com>" is one word).
pub(crate) fn split_words(line: &str) -> Result<Vec<String>, String> {
    let (mut words, mut word, mut quote, mut any) = (Vec::new(), String::new(), None, false);
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                any = false;
            }
            (None, c) => word.push(c),
        }
    }
    if let Some(q) = quote {
        return Err(format!("a {q} isn't closed"));
    }
    if any || !word.is_empty() {
        words.push(word);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_splits_as_a_shell_splits_it() {
        assert_eq!(split_words(r#"ops --from "Treasury <t@acme.com>" --to '*@acme.com'  x"#).unwrap(), ["ops", "--from", "Treasury <t@acme.com>", "--to", "*@acme.com", "x"]);
        assert_eq!(split_words(r#"a "" b"#).unwrap(), ["a", "", "b"], "an empty quoted word is a word");
        assert!(split_words(r#"a "b"#).is_err());
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn options_take_their_values_and_words_stay_in_order() {
        let a = Args::parse(&strings(&["shop", "--server", "7420", "--revoke", "Clock", "--revoke", "StateWrite(\"a\")", "--json", "x"]), &["--revoke"], &["--json"]).unwrap();
        assert_eq!(a.words, ["shop", "x"]);
        assert_eq!(a.server(), Some("7420"));
        assert_eq!(a.all("--revoke"), ["Clock", "StateWrite(\"a\")"]);
        assert!(a.flag("--json") && !a.flag("--yes"));
        assert_eq!(Args::parse(&strings(&["--nope"]), &[], &[]).err().as_deref(), Some("no option \"--nope\""));
        assert_eq!(Args::parse(&strings(&["--revoke"]), &["--revoke"], &[]).err().as_deref(), Some("--revoke takes a value"));
        assert_eq!(Args::parse(&strings(&["a", "-h"]), &[], &[]).err().as_deref(), Some(""));
    }

    #[test]
    fn a_table_lines_its_columns_up() {
        let rows = vec![strings(&["slug", "status", "port"]), strings(&["shop", "running", "8101"]), strings(&["ticker-long", "paused", ""])];
        assert_eq!(table(&rows), ["slug         status   port", "shop         running  8101", "ticker-long  paused"]);
    }
}
