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
  with an API token in HOLON_TOKEN.";

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
    let (_, answer) = local::admin_call(who, args.server(), method, path, body)?;
    if answer.trim().is_empty() {
        return Ok(Value::Null);
    }
    crate::wire::read_json(answer.as_bytes()).map_err(|e| {
        eprintln!("{who}: the server's answer isn't JSON: {e}");
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

#[cfg(test)]
mod tests {
    use super::*;

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
