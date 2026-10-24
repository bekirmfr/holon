//! `holon lsp`: a `.holon` file read by an editor as it is typed (docs/L4s/language-server-L4.md),
//! as a Language Server Protocol server on stdin and stdout. What it says is holon-author's
//! (`editing.rs`): each mistake `holon check` finds, what the word under the cursor is, what may
//! be written there, and the text as `holon fmt` writes it. This file frames the messages and converts positions: the
//! language counts columns in characters, the protocol in UTF-16 code units.

use std::collections::HashMap;
use std::io::{BufRead, Write};

use holon_author::editing;
use holon_author::lang::ast::{Pos, Span};
use serde_json::{json, Value};

const USAGE: &str = "usage: holon lsp

A language server on stdin and stdout, for an editor to start (the VS Code extension in
editors/vscode does): each mistake holon check finds, as you type; what a word is, on hover;
what may be written, as completions; and formatting, as holon fmt writes it. It needs no server.";

// ── framing ─────────────────────────────────────────────────────────────────────────────────

/// The next message, or `None` at the end of the input.
fn read_message(input: &mut impl BufRead) -> std::io::Result<Option<Value>> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().ok();
        }
    }
    let mut body = vec![0; length.unwrap_or(0)];
    input.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

fn write_message(output: &mut impl Write, message: &Value) -> std::io::Result<()> {
    let body = message.to_string();
    write!(output, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    output.flush()
}

// ── positions ───────────────────────────────────────────────────────────────────────────────

fn line_text(text: &str, line: u32) -> &str {
    text.split('\n').nth(line as usize).map(|l| l.strip_suffix('\r').unwrap_or(l)).unwrap_or("")
}

/// A place in the text as the protocol says it: a 0-based line, and UTF-16 code units into it.
fn to_lsp(text: &str, at: Pos) -> Value {
    let line = at.line.saturating_sub(1);
    let units: usize = line_text(text, line).chars().take(at.col.saturating_sub(1) as usize).map(char::len_utf16).sum();
    json!({ "line": line, "character": units })
}

/// A place the protocol names, as the language counts it.
fn from_lsp(text: &str, position: &Value) -> Pos {
    let line = position["line"].as_u64().unwrap_or(0) as u32;
    let mut left = position["character"].as_u64().unwrap_or(0) as usize;
    let mut col = 1;
    for c in line_text(text, line).chars() {
        if left < c.len_utf16() {
            break;
        }
        left -= c.len_utf16();
        col += 1;
    }
    Pos { line: line + 1, col }
}

fn range(text: &str, span: Span) -> Value {
    // A mistake at a point (where the parser stopped) is shown on the character there.
    let end = if span.end == span.start { Pos { col: span.start.col + 1, ..span.start } } else { span.end };
    json!({ "start": to_lsp(text, span.start), "end": to_lsp(text, end) })
}

/// The whole text, as a range.
fn everything(text: &str) -> Value {
    let lines = text.split('\n').count() as u32;
    let last = line_text(text, lines - 1).chars().map(char::len_utf16).sum::<usize>();
    json!({ "start": { "line": 0, "character": 0 }, "end": { "line": lines - 1, "character": last } })
}

/// The protocol's number for what a completion offers.
fn kind(k: editing::Kind) -> u8 {
    match k {
        editing::Kind::Member => 2,
        editing::Kind::Function => 3,
        editing::Kind::Variable => 6,
        editing::Kind::Module => 9,
        editing::Kind::Keyword => 14,
    }
}

// ── the server ──────────────────────────────────────────────────────────────────────────────

/// The texts the editor has open, by URI.
#[derive(Default)]
struct Server {
    texts: HashMap<String, String>,
    shut_down: bool,
}

fn diagnostics(uri: &str, text: &str) -> Value {
    let found: Vec<Value> = editing::mistakes(text).into_iter().map(|m| json!({ "range": range(text, m.span), "severity": 1, "source": "holon", "message": m.message })).collect();
    json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": { "uri": uri, "diagnostics": found } })
}

impl Server {
    /// What to send for `message`: its answer if it asks one, and any notifications.
    fn handle(&mut self, message: &Value) -> Vec<Value> {
        let method = message["method"].as_str().unwrap_or("");
        let params = &message["params"];
        let uri = params["textDocument"]["uri"].as_str().unwrap_or("").to_string();
        let answer = |result: Value| json!({ "jsonrpc": "2.0", "id": message["id"], "result": result });
        match method {
            "initialize" => vec![answer(json!({
                "capabilities": {
                    "textDocumentSync": { "openClose": true, "change": 1 },
                    "hoverProvider": true,
                    "completionProvider": { "triggerCharacters": ["."] },
                    "documentFormattingProvider": true,
                },
                "serverInfo": { "name": "holon", "version": env!("CARGO_PKG_VERSION") },
            }))],
            "textDocument/didOpen" => {
                let text = params["textDocument"]["text"].as_str().unwrap_or("").to_string();
                let said = diagnostics(&uri, &text);
                self.texts.insert(uri, text);
                vec![said]
            }
            "textDocument/didChange" => {
                // Whole text each time (the sync kind above): the last change is the text.
                let Some(text) = params["contentChanges"].as_array().and_then(|c| c.last()).and_then(|c| c["text"].as_str()) else { return vec![] };
                let said = diagnostics(&uri, text);
                self.texts.insert(uri, text.to_string());
                vec![said]
            }
            "textDocument/didClose" => {
                self.texts.remove(&uri);
                vec![json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": { "uri": uri, "diagnostics": [] } })]
            }
            "textDocument/hover" => {
                let text = self.texts.get(&uri).map(String::as_str).unwrap_or("");
                let found = editing::hover(text, from_lsp(text, &params["position"]));
                vec![answer(found.map_or(Value::Null, |h| json!({ "contents": { "kind": "markdown", "value": h.text }, "range": range(text, h.span) })))]
            }
            "textDocument/completion" => {
                let text = self.texts.get(&uri).map(String::as_str).unwrap_or("");
                let items: Vec<Value> = editing::complete(text, from_lsp(text, &params["position"]))
                    .into_iter()
                    .map(|c| {
                        let mut item = json!({ "label": c.label, "kind": kind(c.kind), "detail": c.detail });
                        if let Some(doc) = c.doc {
                            item["documentation"] = json!({ "kind": "markdown", "value": doc });
                        }
                        item
                    })
                    .collect();
                vec![answer(json!(items))]
            }
            "textDocument/formatting" => {
                let text = self.texts.get(&uri).map(String::as_str).unwrap_or("");
                // A text with a mistake is left as it is: the diagnostic already says why.
                let edits = match editing::format(text) {
                    Ok(formatted) if formatted != text => json!([{ "range": everything(text), "newText": formatted }]),
                    _ => json!([]),
                };
                vec![answer(edits)]
            }
            "shutdown" => {
                self.shut_down = true;
                vec![answer(Value::Null)]
            }
            _ if message.get("id").is_some_and(|id| !id.is_null()) => {
                vec![json!({ "jsonrpc": "2.0", "id": message["id"], "error": { "code": -32601, "message": format!("holon lsp doesn't answer {method}") } })]
            }
            // Other notifications (initialized, $/cancelRequest, …) need nothing.
            _ => vec![],
        }
    }
}

/// Answer messages from `input` until `exit` or its end; the exit code the protocol asks for.
fn serve(mut input: impl BufRead, mut output: impl Write) -> std::io::Result<i32> {
    let mut server = Server::default();
    while let Some(message) = read_message(&mut input)? {
        if message["method"] == "exit" {
            return Ok(if server.shut_down { 0 } else { 1 });
        }
        for reply in server.handle(&message) {
            write_message(&mut output, &reply)?;
        }
    }
    Ok(if server.shut_down { 0 } else { 1 })
}

/// `holon lsp`.
pub(crate) fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("--stdio") => {}
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            return 0;
        }
        Some(a) => {
            eprintln!("holon lsp: {a}?\n\n{USAGE}");
            return 2;
        }
    }
    match serve(std::io::stdin().lock(), std::io::stdout().lock()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("holon lsp: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLUGS: &str = include_str!("../../holon-author/samples/slugs.holon");

    fn framed(messages: &[Value]) -> Vec<u8> {
        let mut bytes = vec![];
        for m in messages {
            write_message(&mut bytes, m).unwrap();
        }
        bytes
    }

    /// Every message the server sent for `messages`, and its exit code.
    fn session(messages: &[Value]) -> (Vec<Value>, i32) {
        let mut out = vec![];
        let code = serve(std::io::Cursor::new(framed(messages)), &mut out).unwrap();
        let mut input = std::io::Cursor::new(out);
        let mut sent = vec![];
        while let Some(m) = read_message(&mut input).unwrap() {
            sent.push(m);
        }
        (sent, code)
    }

    fn request(id: u64, method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    }

    fn notify(method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "method": method, "params": params })
    }

    #[test]
    fn an_editor_opens_changes_hovers_formats_and_shuts_down() {
        let uri = "file:///slugs.holon";
        let bad = "flow f\nport 8200\n\nroute GET /a (req) {\n  respond { a: \"😀\", b: nope }\n}\n";
        let messy = "flow f\nport 8200\nroute GET /a (req) {\n    respond   204\n}";
        let (sent, code) = session(&[
            request(1, "initialize", json!({ "capabilities": {} })),
            notify("initialized", json!({})),
            notify("textDocument/didOpen", json!({ "textDocument": { "uri": uri, "languageId": "holon", "version": 1, "text": SLUGS } })),
            request(2, "textDocument/hover", json!({ "textDocument": { "uri": uri }, "position": { "line": 6, "character": 22 } })),
            request(20, "textDocument/completion", json!({ "textDocument": { "uri": uri }, "position": { "line": 23, "character": 16 } })),
            notify("textDocument/didChange", json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": bad }] })),
            request(3, "textDocument/formatting", json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 2, "insertSpaces": true } })),
            notify("textDocument/didChange", json!({ "textDocument": { "uri": uri, "version": 3 }, "contentChanges": [{ "text": messy }] })),
            request(4, "textDocument/formatting", json!({ "textDocument": { "uri": uri }, "options": {} })),
            request(5, "textDocument/definition", json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 0 } })),
            notify("textDocument/didClose", json!({ "textDocument": { "uri": uri } })),
            request(6, "shutdown", Value::Null),
            notify("exit", Value::Null),
        ]);
        assert_eq!(code, 0);
        assert_eq!(sent[0]["result"]["capabilities"]["hoverProvider"], true);
        assert_eq!(sent[1]["params"]["diagnostics"], json!([]), "the sample has no mistakes");
        // `lowercase` on line 7 (0-based 6), from column 22.
        let hover = &sent[2]["result"];
        assert!(hover["contents"]["value"].as_str().unwrap().contains("lowercase("), "{hover}");
        assert_eq!(hover["range"], json!({ "start": { "line": 6, "character": 20 }, "end": { "line": 6, "character": 29 } }));
        // After `posts.` on line 24 (0-based 23): the collection's methods, as members.
        let offered = sent[3]["result"].as_array().unwrap();
        assert_eq!(offered.iter().map(|c| c["label"].as_str().unwrap()).collect::<Vec<_>>(), ["get", "put", "delete", "list"]);
        assert_eq!(offered[0]["kind"], 2);
        let sent = [&sent[..3], &sent[4..]].concat();
        // The mistake: `nope`, the 24th character, after an emoji that is one character and two
        // UTF-16 units.
        let d = &sent[3]["params"]["diagnostics"][0];
        assert_eq!(d["message"], "unknown name \"nope\"");
        assert_eq!(d["range"], json!({ "start": { "line": 4, "character": 24 }, "end": { "line": 4, "character": 28 } }));
        assert_eq!(sent[4]["result"], json!([]), "a text with a mistake isn't formatted");
        assert_eq!(sent[6]["result"], json!([{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 4, "character": 1 } }, "newText": "flow f\nport 8200\n\nroute GET /a (req) {\n  respond 204\n}\n" }]));
        assert_eq!(sent[7]["error"]["code"], -32601);
        assert_eq!(sent[8]["params"]["diagnostics"], json!([]), "closed: its diagnostics cleared");
        assert_eq!(sent[9]["result"], Value::Null);
        assert_eq!(sent.len(), 10);
    }

    #[test]
    fn exit_without_shutdown_is_a_failure() {
        assert_eq!(session(&[notify("exit", Value::Null)]).1, 1);
    }

    #[test]
    fn positions_count_utf16_units_both_ways() {
        let text = "a\n x😀y\n";
        let at = Pos { line: 2, col: 4 }; // the `y`
        assert_eq!(to_lsp(text, at), json!({ "line": 1, "character": 4 }));
        assert_eq!(from_lsp(text, &json!({ "line": 1, "character": 4 })), at);
        assert_eq!(everything("a\nbc"), json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 2 } }));
    }
}
