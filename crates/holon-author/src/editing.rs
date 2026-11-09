//! What an editor asks of a flow's text as it is typed, a `NAME.holon.ts` file in TypeScript's
//! syntax (docs/L4s/language-server-L4.md, docs/L4s/typescript-syntax-L4.md §13): its mistakes,
//! with their spans; what the word under the cursor is; what may be written there; its colours;
//! and the text in the canonical form. `holon lsp` speaks these to an editor, the web editor asks
//! them through the wasm bridge and the terminal editor colours with them; the answers are the ones
//! `holon check` and `holon fmt` give, and the reference `holon mcp` gives an agent, so an editor,
//! the command line and an agent read a flow alike.
//!
//! A text is read here by a scanner of its own, which never stops: a string not closed, a comment
//! not ended or a line half typed is read as far as it goes. What the text imports, binds and
//! hands its handlers is read from its words ([`Outline`]), so a text that doesn't compile yet is
//! still coloured, said and completed; what only a compiled text knows (a binding's node, a
//! function's outputs, where a secret is sent) comes from the TypeScript reader (`lang::ts`).

use std::collections::{BTreeMap, BTreeSet};

use holon_core::MapResolver;

use crate::agent;
use crate::commands;
use crate::composites::Names;
use crate::lang::ast::{HeaderKind, Item, PortDecl, Pos, Span};
use crate::lang::ts;
use crate::text;

/// Something wrong with a text, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct Mistake {
    pub span: Span,
    pub message: String,
}

/// What a hover says, and the span of the word it is about.
#[derive(Clone, Debug, PartialEq)]
pub struct Hover {
    pub span: Span,
    /// Markdown.
    pub text: String,
}

/// The text's start, where a mistake with no place of its own is shown.
const START: Span = Span { start: Pos { line: 1, col: 1 }, end: Pos { line: 1, col: 1 } };

/// The span of the `flow(…)` call, where a mistake about the whole flow is shown; else the start.
fn flow_span(source: &str) -> Span {
    let at = ts::parse(source).ok().and_then(|f| f.headers.iter().find(|h| matches!(h.kind, HeaderKind::Flow(_))).map(|h| h.span));
    at.unwrap_or(START)
}

/// Every mistake `holon check` finds: the first one the text can't be compiled past, with its
/// span; or, compiled, what is wrong with the flow as a whole (at its `flow(…)` call).
pub fn mistakes(source: &str) -> Vec<Mistake> {
    let compiled = match ts::compile(source) {
        Ok(c) => c,
        // A mistake of the whole text (it has no `flow(…)`) has no place: it is said at the start.
        Err(e) => return vec![Mistake { span: if e.span.start.line == 0 { START } else { e.span }, message: e.message }],
    };
    let mut store = MapResolver::new();
    text::restore(&compiled.doc, &mut store, &Names::new());
    let span = flow_span(source);
    commands::problems(&compiled, &mut store).into_iter().map(|message| Mistake { span, message }).collect()
}

/// The text in the canonical form, as `holon fmt` writes it; or why it can't be, at its place
/// (`5:23: unknown name "nope"`).
pub fn format(source: &str) -> Result<String, String> {
    crate::lang::format::format(source).map_err(|e| e.to_string())
}

// ── what the words are ──────────────────────────────────────────────────────────────────────

/// Holon's vocabulary, imported from `"holon"` (docs/manual/language/reference.md).
const VOCABULARY: &[&str] = &["flow", "route", "every", "on", "action", "js", "respond", "status", "stripe", "event"];

/// The language's words, TypeScript's and Holon's vocabulary, said in a line each.
const KEYWORDS: &[(&str, &str)] = &[
    ("import", "`import notes from \"collection:notes\"`: names something the flow uses from outside a run, its source saying what: `\"holon\"` (the vocabulary and the leaves), `\"secret:NAME\"`, `\"state\"`, `\"collection:NAME\"`, `\"folder:NAME\"`, `\"database:NAME\"`, `\"channel:NAME\"`, `\"host:HOST\"`, `\"holon:clock\"`, `\"holon:random\"` or `\"flow:SLUG\"`. An import names; it grants nothing: what the flow may do is worked out from what it does with what it imports."),
    ("with", "`import key from \"secret:NAME\" with { optional: \"true\" }`: a secret the flow can run without; the parts that need it have no value when it isn't set."),
    ("flow", "`flow(\"name\", { port: 8080 })`: what the flow is, once, after its imports: the name it deploys as, and its deploy settings (`port`, `keep`, `limits`, `browsers`)."),
    ("route", "`route.post(\"/path/:id\", (req) => answer, { resumes, log })`: answers requests on the flow's port, by `get`, `post`, `put`, `patch` or `delete`; `:id` in the path is read as `req.params.id`. `req` names the request (any name; destructured, or left out when the route reads nothing of it); `async` goes on one that `await`s."),
    ("every", "`every(\"1h\", (now) => { … })`: a run every period, a whole number of seconds from `\"1s\"` to `\"366d\"`, one clock per period; `now` is the tick's time, left out when it isn't read."),
    ("on", "`on.file.created(inbox, (f) => { … }, { matching: \"*.csv\" })`: a run for each file new or changed in the folder `inbox` imports, once it stops changing; `on.file.deleted`, for each file removed. `f` is the file's handle. `on.mail(remit, (m) => { … })`: a run for each message that comes to the mailbox a mail is bound to. `on.feed(prices, (e) => { … })`: a run for each event a feed sends, one at a time."),
    ("action", "`export const reset = action((input) => { … })`: a run by hand, by its name, `input` what was sent (`{}` for nothing); it answers with what it returns, a value alone. It is exported, as nothing else could reach it; a route starts one by calling it without `await`, `const started = reset(req.json)`."),
    ("js", "`const page = js(function html(past) { … }, { fuel, memory, outputs })`: a function whose body is JavaScript, run as written with no clock, randomness or I/O, under its budgets (200M instructions and 16 MiB when not written). Its text is its identity: `holon fmt` leaves it alone."),
    ("respond", "`return respond(body, { status: 201, headers: { … }, stream: true })`: a route's answer with its status, its headers (a record of texts), or streamed. Like `status(…)`, it is a route's answer and nothing else: return it."),
    ("status", "`return status(201, body)`: a route's answer with that status (200 to 599); `status(204)`, the status alone; `body ?? status(404)`, 404 when `body` has no value."),
    ("event", "`await event(\"paid\", { expires: \"1d\" })`: parks an action's run until the event arrives; a route with `{ resumes: \"paid\" }` delivers it. A route doesn't wait: it starts the action that does."),
    ("export", "`export const reset = action(…)`: only actions are exported, as nothing else reaches the flow from outside."),
    ("const", "`const name = expr`: binds a name, one value wherever it is read. At the top level, only literals (`const limit = 50`), functions and `js()` functions."),
    ("let", "`let name = expr`: a name that may be bound again; each assignment is a new value, and where branches assign it, the value is the branch taken's."),
    ("var", "`var name = expr`: read as `let`; `holon fmt` writes `const`, or `let` where it is assigned again."),
    ("function", "`function slugify(text) { … return slug }`: a piece of flow placed as one node where it is called; its outputs are what it returns (`return slug` gives `slug`, `return { a, b }` gives `a` and `b`)."),
    ("async", "`async (req) => { … }`: a handler that `await`s, as TypeScript requires."),
    ("await", "`await api.get(path)`: a request, made when the run gets to it; also another flow's action called, `await charge(x)`, and `await event(…)`, which parks an action's run. A handler that awaits is `async`."),
    ("return", "`return value`: what a route or an action answers; several `return`s are one answer, each chosen where its branch is taken. In a clock or a file handler, it only ends the run."),
    ("if", "`if (c) … else …`: writes and calls in a branch happen only when it is taken. `if (x !== undefined) { … }` ties them to `x`: they happen once `x` has a value. Only the branch taken runs."),
    ("switch", "`switch (s) { case \"a\": … default: … }`: the first case equal to `s` is chosen, and only its body runs; each body returns or assigns, with no fall-through. With no case equal and no `default`, no value."),
    ("delete", "`delete notes[key]`: removes an item from a collection, or a file from a folder, when the run finishes."),
    ("undefined", "`undefined`: no value: as a branch, `c ? a : undefined`, and in `x !== undefined`. Absence isn't a value: it can't be stored or compared."),
];

/// Words said with another's line.
const SAID_WITH: &[(&str, &str)] = &[("from", "import"), ("else", "if"), ("case", "switch"), ("default", "switch")];

/// What a flow imports from outside a run, by its kind: each use is a permission the flow asks for.
const NOUNS: &[(&str, &str)] = &[
    ("secret", "A secret, `import key from \"secret:NAME\"`, set by an admin under the deployment's ⚙ and never in the run log; `with { optional: \"true\" }`, one the flow can run without. One permission: reading it; a route that answers with it, or with what is computed from it, asks for another."),
    ("state", "`state.key`: the flow's own value under `key` (`import state from \"state\"`), as the run began; `state.key = v` writes it when the run finishes. Each key read or written is one permission."),
    ("now", "`now` (`import { now } from \"holon:clock\"`): the time the run started, in seconds since the epoch, recorded with the run so a replay sees the same; in a clock, the tick's. A permission: reading the clock."),
    ("random", "`random` (`import { random } from \"holon:random\"`): one random number for the run, recorded so a replay sees the same. A permission: randomness."),
    ("random_bytes", "`random_bytes` (`import { random_bytes } from \"holon:random\"`): the run's 32 random bytes, recorded so a replay sees the same: what `pgp_encrypt` draws a message's randomness from. A permission: randomness."),
    ("collection", "A named store of items, `import notes from \"collection:notes\"`: `notes[key]`, `notes[key] = v` and `delete notes[key]` (or `notes.get(k)`, `notes.put(k, v)`, `notes.delete(k)`), and `notes.list({ prefix: p, after: a, limit: 50 })`, a page of items in key order. One permission for each use."),
    ("folder", "Files on the host, in a folder an admin binds to the name, `import inbox from \"folder:inbox\"`: `inbox[file]` reads a file's text, `inbox[file] = text` writes it when the run finishes (`inbox.create(file, text)` only if no file has that name), `delete inbox[file]` deletes it, `inbox.list({ prefix: p, after: a, limit: 50 })` lists files; `inbox.file(name)` is a handle to a file of any size, and `inbox.lines(…)` and `inbox.slice(…)` read part of one. One permission to read, one to write."),
    ("database", "A database an admin binds on the host, `import shop from \"database:shop\"`. `shop.query(\"select … where id = $1\", id)` reads rows (a list of records by column name), `shop.one(…)` the first row or nothing, `shop.exec(\"update … where id = $1\", id, { expect: 1 })` changes rows when the run finishes, in one transaction with its other writes there (`expect`: exactly that many, or nothing lands). The SQL is written in quotes; its tables are the permission, one to read each, one to write each. Values go after it as $1, $2, …, never into it. A document database (MongoDB) takes records instead: `notes.find(\"notes\", { owner: who }, { sort: { at: -1 }, limit: 20 })` reads documents, `notes.find_one(…)` the first, and `notes.insert(\"notes\", { … })`, `notes.update(\"notes\", { _id: id }, { \"$set\": { text: t } }, { expect: 1 })` and `notes.delete(\"notes\", { … })` write when the run finishes. The collection is the permission; each computed value goes in as a value, never as an operator."),
    ("channel", "Packs the flow sends to whoever subscribes, `import updates from \"channel:updates\"`: `updates.send(v)` sends one once the run is over and its writes have landed; a route answers with `updates.subscribe()`, a stream of server-sent events (`{ from: \"kept\" }`, the ones it keeps first); `updates.pipe(…)` passes a call's answer on as it arrives. One permission to send, one to let callers subscribe."),
    ("mail", "Mail sent through a mail resource an admin binds on the host, `import ops from \"mail:ops\"`: `ops.send({ to: \"approvals@acme.com\", cc, subject, text, html, attachments: { \"payout.csv\": csv } })` sends it once the run is over and its writes have landed, from the binding's address, never the flow's. Each recipient written in quotes is a permission of its own; a computed one is one permission, held to the addresses the binding allows. At most 50 recipients and 10 MiB."),
    ("feed", "A feed an admin binds on the host, `import prices from \"feed:prices\"`: a stream of events (server-sent events) the host keeps a connection to. `on.feed(prices, (e) => { … })` runs once for each event, one at a time, in the order they came: `e.data` (a value when it is JSON, else its text), `e.id` and `e.event`. The host keeps where the feed is, so a reconnect resumes after the last event run on, and none runs twice. One permission: running on its events."),
    ("host", "`await api.get(path, { headers: h, body: b })`, `import api from \"host:api.example.com\"`: a request to the host over HTTPS; also `post`, `put`, `patch`, `delete` and `head`. The host is the permission; the answer, `{status, headers, body}`, is recorded with the run. `file: handle` in place of `body` sends a file's bytes, streamed by the server."),
    ("flow", "`import { charge } from \"flow:billing\"`: an action of another deployed flow. `await charge(x)` runs it and answers `{answer, run}`, or `{error, message}` when it didn't run; `charge(x)` without `await` starts it. One permission: running it."),
    ("stripe", "`stripe.verify(req, secret, now, { tolerance: 300 })`: the request as a verified Stripe webhook (`.event`), or no value."),
    ("JSON", "`JSON.parse(text)` and `JSON.stringify(x)`: JSON in and out, the leaves `json_parse` and `json_stringify`."),
    ("Object", "`Object.keys(r)`: a record's keys, the leaf `keys`."),
];

/// What a route's request holds.
const REQUEST: &str = "the route's request: `.params` (the path's `:names`), `.query`, `.headers`, `.body` (text) and `.json` (the body parsed).";
/// What a file handler's parameter is.
const FILE: &str = "the handle of the file that started the run: `.folder`, `.name`, `.size`, `.hash`, `.modified` and `.event`, no bytes.";
/// What an action's parameter is.
const INPUT: &str = "what the action was sent: a JSON value, `{}` for nothing.";
/// What a clock's parameter is.
const TICK: &str = "the time of the tick, in seconds since the epoch.";

/// A value's methods that are leaves (docs/manual/language/reference.md, Calls).
const METHOD_LEAVES: &[(&str, &str)] = &[
    ("toUpperCase", "uppercase"),
    ("toLowerCase", "lowercase"),
    ("toLocaleUpperCase", "uppercase"),
    ("toLocaleLowerCase", "lowercase"),
    ("charAt", "text_at"),
    ("charCodeAt", "code_at"),
    ("codePointAt", "code_at"),
    ("startsWith", "text_contains"),
    ("endsWith", "text_contains"),
    ("substring", "slice"),
    ("trim", "trim"),
    ("trimStart", "trim"),
    ("trimEnd", "trim"),
    ("padStart", "pad"),
    ("padEnd", "pad"),
    ("repeat", "repeat"),
    ("replace", "replace"),
    ("replaceAll", "replace"),
    ("normalize", "normalize"),
    ("isWellFormed", "is_well_formed"),
    ("toWellFormed", "concat"),
    ("split", "split"),
    ("join", "join"),
    ("indexOf", "index_of"),
    ("map", "map"),
    ("reduce", "fold"),
    ("reduceRight", "fold"),
    ("filter", "filter"),
    ("some", "any"),
    ("every", "all"),
    ("find", "find"),
    ("findIndex", "find_index"),
    ("findLast", "find_last"),
    ("findLastIndex", "find_last_index"),
    ("flatMap", "flat"),
    ("toSorted", "sort_by"),
    ("at", "at"),
    ("includes", "includes"),
    ("lastIndexOf", "last_index_of"),
    ("toReversed", "reverse"),
    ("slice", "slice"),
    ("toSpliced", "to_spliced"),
    ("with", "with_item"),
    ("fill", "fill"),
    ("copyWithin", "copy_within"),
    ("flat", "flat"),
    ("keys", "indices"),
    ("entries", "entries"),
    ("hasOwnProperty", "has"),
    ("getTime", "date_part"),
    ("getUTCFullYear", "date_part"),
    ("getUTCMonth", "date_part"),
    ("getUTCDate", "date_part"),
    ("getUTCDay", "date_part"),
    ("getUTCHours", "date_part"),
    ("getUTCMinutes", "date_part"),
    ("getUTCSeconds", "date_part"),
    ("getUTCMilliseconds", "date_part"),
    ("toISOString", "date_text"),
    ("toUTCString", "date_text"),
    ("setUTCFullYear", "date_set"),
    ("setUTCMonth", "date_set"),
    ("setUTCDate", "date_set"),
    ("setUTCHours", "date_set"),
    ("setUTCMinutes", "date_set"),
    ("setUTCSeconds", "date_set"),
    ("setUTCMilliseconds", "date_set"),
];

/// A flow's setting or a handler's option, by the call it is in (`flow`, `route`, …) and its key.
fn option(call: &str, key: &str) -> Option<&'static str> {
    Some(match (call, key) {
        ("flow", "port") => "`port: 8102`: the port the flow's server answers its routes on.",
        ("flow", "keep") => "`keep: { runs: 500, days: 30 }`: how many runs the deployment's log keeps, and for how many days; either alone works. Fewer than the host's default asks nobody; more asks for approval, up to the host's ceiling.",
        ("flow", "limits") => "`limits: { run_budget_ms: \"1m\", request_timeout_ms: \"50s\", max_requests: 100 }`: any other limit of the deploy settings, by its name there. A `_ms` or `_seconds` limit takes a duration as text or a whole number in its unit, the others a whole number. Below the host's default asks nobody; above asks for approval, up to the host's ceiling.",
        ("flow", "browsers") => "`browsers: [\"https://app.example\"]`: the web pages it answers from other sites (CORS), or `\"*\"` for any.",
        ("route" | "every" | "on" | "action", "log") => "`{ log: \"lazy\" }`: how the handler's runs are recorded: each a line of the deployment's log when left out (strict); `\"lazy\"`, recorded and on disk up to a second after the answer; `\"unrecorded\"`, only counted, which a person approves at deploy.",
        ("route", "resumes") => "`{ resumes: \"paid\" }`: the route delivers the event a run parked at `await event(\"paid\")` waits for, the request carrying the token its start was answered with, `started.waiting.paid`.",
        ("on", "matching") => "`{ matching: \"*.csv\" }`: only the files whose names match (`*` and `?` within a folder, `**/` any number of folders, letters either case).",
        ("event", "expires") => "`{ expires: \"1d\" }`: how long the run waits for the event.",
        ("js", "fuel") => "`fuel: 50_000_000`: the instructions a call may run (200M when not written); the deploy asks for it, and the server's policy caps it.",
        ("js", "memory") => "`memory: 16_777_216`: the memory a call may take (16 MiB when not written); the deploy asks for it, and the server's policy caps it.",
        ("js", "outputs") => "`outputs: [\"best\", \"distance\"]`: the function returns a record with those fields, each an output.",
        _ => return None,
    })
}

fn fenced(sig: &str, doc: &str) -> String {
    if doc.is_empty() {
        format!("```ts\n{sig}\n```")
    } else {
        format!("```ts\n{sig}\n```\n{doc}")
    }
}

fn line_of<'a>(table: &'a [(&str, &str)], word: &str) -> Option<&'a str> {
    table.iter().find(|(w, _)| *w == word).map(|(_, l)| *l)
}

/// A keyword's line, a word said with another's by that one's.
fn keyword(word: &str) -> Option<&'static str> {
    line_of(KEYWORDS, line_of(SAID_WITH, word).unwrap_or(word))
}

fn noun(kind: &str) -> &'static str {
    line_of(NOUNS, kind).unwrap_or("")
}

/// A leaf or template, as the reference says it: its signature, then what it does.
fn leaf(word: &str) -> Option<String> {
    let said = agent::reference(Some(word)).ok()?;
    if said.starts_with("//") {
        return None; // a sample, by its name
    }
    let (sig, doc) = said.split_once("\n\n").unwrap_or((&said, ""));
    Some(fenced(sig, doc))
}

/// `a`, `` `a` and `b` ``, `` `a`, `b` and `c` ``.
fn listed(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
    match quoted.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => quoted.concat(),
    }
}

/// A function's outputs, said: `its output is `slug``.
fn outputs(ports: &[PortDecl]) -> String {
    let names: Vec<String> = ports.iter().map(|p| p.name.text.clone()).collect();
    if names.len() == 1 { format!("its output is {}", listed(&names)) } else { format!("its outputs are {}", listed(&names)) }
}

fn covers(span: Span, at: Pos) -> bool {
    let after_start = (at.line, at.col) >= (span.start.line, span.start.col);
    let before_end = (at.line, at.col) < (span.end.line, span.end.col);
    after_start && before_end
}

fn before(a: Pos, b: Pos) -> bool {
    (a.line, a.col) < (b.line, b.col)
}

// ── reading the words ───────────────────────────────────────────────────────────────────────

/// What a stretch of a text is, as the scanner reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lex {
    Word,
    Number,
    /// A string from its quote, a template's text between its parts, or a regular expression.
    Str,
    /// `\n` in a string.
    Escape,
    /// A template's `${`, and the `}` that closes it.
    Interp,
    Comment,
    /// An operator or punctuation: `=>`, `?.`, `(`, `,`.
    Punct,
}

#[derive(Clone, Debug)]
struct Lexeme {
    lex: Lex,
    text: String,
    span: Span,
    /// The cursor at its end is still in it: a line comment, a string not closed yet, a string's
    /// part before an escape or a `${`.
    open: bool,
}

/// TypeScript's operators and punctuation of more than one character, the longest first.
const PUNCTUATION: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=", "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
    "**", "<<", ">>",
];

/// The punctuation coloured as an operator.
const OPERATORS: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=", "<=", ">=", "&&", "||", "??", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "**",
    "<<", ">>", "+", "-", "*", "/", "%", "<", ">", "!", "=", "?", "&", "|", "^", "~",
];

/// The words a `/` after which starts a regular expression, not a division.
const REGEX_AFTER: &[&str] = &["return", "typeof", "case", "in", "of", "delete", "void", "throw", "new", "else", "do", "await", "yield"];

/// The text's words, in order, never stopping: what isn't closed runs as far as it may (a string
/// to its line's end, a template or a block comment to the text's).
struct Scanner {
    chars: Vec<char>,
    i: usize,
    at: Pos,
    out: Vec<Lexeme>,
}

impl Scanner {
    fn peek(&self, n: usize) -> Option<char> {
        self.chars.get(self.i + n).copied()
    }

    fn bump(&mut self) {
        if let Some(&c) = self.chars.get(self.i) {
            self.at = if c == '\n' { Pos { line: self.at.line + 1, col: 1 } } else { Pos { col: self.at.col + 1, ..self.at } };
            self.i += 1;
        }
    }

    /// Whether the line ends `n` characters on (a `\r` before its `\n` isn't on it).
    fn eol(&self, n: usize) -> bool {
        match self.peek(n) {
            None | Some('\n') => true,
            Some('\r') => matches!(self.peek(n + 1), None | Some('\n')),
            _ => false,
        }
    }

    fn here(&self) -> (usize, Pos) {
        (self.i, self.at)
    }

    /// From `from` to here, as `lex`, unless it is empty.
    fn emit(&mut self, from: (usize, Pos), lex: Lex, open: bool) {
        if self.i > from.0 {
            let text = self.chars[from.0..self.i].iter().collect();
            self.out.push(Lexeme { lex, text, span: Span { start: from.1, end: self.at }, open });
        }
    }

    /// Code to the text's end, or (`inside` a template's `${…}`) to the `}` that closes it:
    /// whether it was closed.
    fn code(&mut self, inside: bool) -> bool {
        let mut depth = 0usize;
        while let Some(c) = self.peek(0) {
            let from = self.here();
            match c {
                c if c.is_whitespace() => self.bump(),
                '/' if self.peek(1) == Some('/') => {
                    while !self.eol(0) {
                        self.bump();
                    }
                    self.emit(from, Lex::Comment, true);
                }
                '/' if self.peek(1) == Some('*') => {
                    self.bump();
                    self.bump();
                    let mut closed = false;
                    while let Some(c) = self.peek(0) {
                        self.bump();
                        if c == '*' && self.peek(0) == Some('/') {
                            self.bump();
                            closed = true;
                            break;
                        }
                    }
                    self.emit(from, Lex::Comment, !closed);
                }
                '"' | '\'' => self.string(c),
                '`' => self.template(),
                '/' if self.regex_may_start() => self.regex(),
                '}' if inside && depth == 0 => {
                    self.bump();
                    self.emit(from, Lex::Interp, false);
                    return true;
                }
                c if c.is_ascii_digit() || (c == '.' && self.peek(1).is_some_and(|d| d.is_ascii_digit())) => self.number(),
                c if c.is_alphabetic() || c == '_' || c == '$' => {
                    while self.peek(0).is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$') {
                        self.bump();
                    }
                    self.emit(from, Lex::Word, false);
                }
                _ => {
                    let n = PUNCTUATION.iter().find(|p| p.chars().enumerate().all(|(k, pc)| self.peek(k) == Some(pc))).map_or(1, |p| p.len());
                    match c {
                        '{' => depth += 1,
                        '}' => depth = depth.saturating_sub(1),
                        _ => {}
                    }
                    for _ in 0..n {
                        self.bump();
                    }
                    self.emit(from, Lex::Punct, false);
                }
            }
        }
        false
    }

    /// `\` and the character it escapes (not a line's end).
    fn escape(&mut self) {
        let from = self.here();
        self.bump();
        if !self.eol(0) {
            self.bump();
        }
        self.emit(from, Lex::Escape, true);
    }

    /// A string from its quote, its escapes apart: to its closing quote, or its line's end.
    fn string(&mut self, quote: char) {
        let mut from = self.here();
        self.bump();
        loop {
            if self.eol(0) {
                return self.emit(from, Lex::Str, true);
            }
            match self.peek(0) {
                Some(c) if c == quote => {
                    self.bump();
                    return self.emit(from, Lex::Str, false);
                }
                Some('\\') => {
                    self.emit(from, Lex::Str, true);
                    self.escape();
                    from = self.here();
                }
                _ => self.bump(),
            }
        }
    }

    /// A template literal from its backtick: its text, its escapes, and each `${…}` read as code;
    /// to its closing backtick, or the text's end.
    fn template(&mut self) {
        let mut from = self.here();
        self.bump();
        loop {
            match self.peek(0) {
                None => return self.emit(from, Lex::Str, true),
                Some('`') => {
                    self.bump();
                    return self.emit(from, Lex::Str, false);
                }
                Some('\\') => {
                    self.emit(from, Lex::Str, true);
                    self.escape();
                    from = self.here();
                }
                Some('$') if self.peek(1) == Some('{') => {
                    self.emit(from, Lex::Str, true);
                    let open = self.here();
                    self.bump();
                    self.bump();
                    self.emit(open, Lex::Interp, false);
                    if !self.code(true) {
                        return;
                    }
                    from = self.here();
                }
                Some(_) => self.bump(),
            }
        }
    }

    /// Whether a `/` here starts a regular expression (in a `js()` body): not after a value (a
    /// name, a number, a string, a closing bracket), where it divides.
    fn regex_may_start(&self) -> bool {
        match self.out.iter().rev().find(|l| l.lex != Lex::Comment) {
            None => true,
            Some(l) => match l.lex {
                Lex::Word => REGEX_AFTER.contains(&l.text.as_str()),
                Lex::Punct => !matches!(l.text.as_str(), ")" | "]" | "}"),
                Lex::Interp => l.text == "${",
                _ => false,
            },
        }
    }

    /// A regular expression from its `/`, with its flags: a string, to its line's end at most.
    fn regex(&mut self) {
        let from = self.here();
        self.bump();
        let mut class = false;
        loop {
            if self.eol(0) {
                return self.emit(from, Lex::Str, true);
            }
            match self.peek(0) {
                Some('\\') => {
                    self.bump();
                    if !self.eol(0) {
                        self.bump();
                    }
                }
                Some('/') if !class => {
                    self.bump();
                    break;
                }
                // (The text's end is a line's end, met above.)
                c => {
                    class = (class || c == Some('[')) && c != Some(']');
                    self.bump();
                }
            }
        }
        while self.peek(0).is_some_and(|c| c.is_ascii_alphabetic()) {
            self.bump();
        }
        self.emit(from, Lex::Str, false);
    }

    /// `86400`, `2.5`, `1e-3`, `1_000_000`, `0xff`.
    fn number(&mut self) {
        let from = self.here();
        let radix = self.peek(0) == Some('0') && matches!(self.peek(1), Some('x' | 'X' | 'b' | 'B' | 'o' | 'O'));
        while let Some(c) = self.peek(0) {
            let fraction = c == '.' && self.peek(1).is_some_and(|d| d.is_ascii_digit());
            let exponent = !radix && matches!(c, '+' | '-') && matches!(self.i.checked_sub(1).and_then(|j| self.chars.get(j)), Some('e' | 'E'));
            if !(c.is_ascii_alphanumeric() || c == '_' || fraction || exponent) {
                break;
            }
            self.bump();
        }
        self.emit(from, Lex::Number, false);
    }
}

/// The text's words, in order.
fn scan(source: &str) -> Vec<Lexeme> {
    let mut s = Scanner { chars: source.chars().collect(), i: 0, at: Pos { line: 1, col: 1 }, out: vec![] };
    s.code(false);
    s.out
}

fn is_punct(l: Option<&&Lexeme>, p: &str) -> bool {
    l.is_some_and(|l| l.lex == Lex::Punct && l.text == p)
}

fn is_dot(l: Option<&&Lexeme>) -> bool {
    is_punct(l, ".") || is_punct(l, "?.")
}

fn word_of<'a>(l: Option<&&'a Lexeme>) -> Option<&'a str> {
    l.filter(|l| l.lex == Lex::Word).map(|l| l.text.as_str())
}

// ── what the text says ──────────────────────────────────────────────────────────────────────

/// What a name imported is (docs/L4s/typescript-syntax-L4.md §3), by its import's source.
#[derive(Clone, Debug, PartialEq)]
enum Imported {
    /// From `"holon"`: a word of its vocabulary.
    Vocab(&'static str),
    /// From `"holon"`: a leaf, by its name.
    Leaf(String),
    Secret { name: String, optional: bool },
    State,
    /// A collection, folder, database or channel: which, and its name.
    Declared(&'static str, String),
    Host(String),
    Now,
    Random,
    RandomBytes,
    /// Another flow's action: `flow:billing`'s `charge`.
    Action { flow: String, action: String },
}

/// What a handler's parameter is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Received {
    Request,
    File,
    Input,
    Tick,
}

impl Received {
    /// What it is, in a line and in full.
    fn said(self) -> (&'static str, &'static str) {
        match self {
            Received::Request => ("a route's request", REQUEST),
            Received::File => ("a file handler's file", FILE),
            Received::Input => ("what an action was sent", INPUT),
            Received::Tick => ("a clock's tick", TICK),
        }
    }
}

/// What a text says, read from its words even when it doesn't compile: what it imports, what its
/// handlers receive, its functions and bindings, and the class of each word that names one.
#[derive(Default)]
struct Outline {
    imports: Vec<(String, Imported)>,
    params: Vec<(String, Received)>,
    functions: Vec<String>,
    bindings: Vec<String>,
    /// The class of each word that defines or imports a name, by its place in the code's words.
    roles: BTreeMap<usize, Class>,
}

impl Outline {
    /// `code`: the text's words, its comments left out.
    fn read(code: &[&Lexeme]) -> Outline {
        let mut o = Outline::default();
        let word = |i: usize| word_of(code.get(i));
        let after_dot = |i: usize| i.checked_sub(1).is_some_and(|j| is_dot(code.get(j)));
        // The imports first, as ES hoists them: they say what the rest's words are.
        let mut i = 0;
        while i < code.len() {
            i = if word(i) == Some("import") && !after_dot(i) { o.import(code, i) } else { i + 1 };
        }
        // A `js()` function's body is JavaScript: the names it binds are its own, not the flow's.
        let mut in_js = vec![false; code.len()];
        for i in 0..code.len() {
            if word(i).is_some_and(|w| o.vocab(w) == Some("js")) && is_punct(code.get(i + 1), "(") {
                let mut depth = 0;
                for (k, l) in code.iter().enumerate().skip(i + 1) {
                    in_js[k] = true;
                    match l.text.as_str() {
                        "(" if l.lex == Lex::Punct => depth += 1,
                        ")" if l.lex == Lex::Punct => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        for (i, &inside) in in_js.iter().enumerate() {
            match word(i) {
                Some("const" | "let" | "var") if !after_dot(i) => o.declaration(code, i + 1, !inside),
                Some("function") if !after_dot(i) => {
                    if let Some(name) = word(i + 1) {
                        o.roles.insert(i + 1, Class::Function);
                        // One written in place (`js(function html(…) …)`) isn't the flow's.
                        let placed = i.checked_sub(1).is_some_and(|j| is_punct(code.get(j), "(") || is_punct(code.get(j), ",") || is_punct(code.get(j), "="));
                        if !placed && !inside {
                            o.functions.push(name.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
        // What each handler receives: `route.get("/p", (req) => …)`, `every("1h", (now) => …)`,
        // `on.file.created(inbox, (f) => …)`, `action((input) => …)`.
        for i in 0..code.len() {
            let Some(w) = word(i).filter(|_| !after_dot(i)) else { continue };
            let call = |n: usize| is_punct(code.get(i + n), "(");
            let comma = |n: usize| is_punct(code.get(i + n), ",");
            let (from, received) = match o.vocab(w) {
                Some("route") if is_dot(code.get(i + 1)) && call(3) && comma(5) => (i + 6, Received::Request),
                Some("every") if call(1) && comma(3) => (i + 4, Received::Tick),
                Some("on") if word(i + 2) == Some("file") && call(5) && comma(7) => (i + 8, Received::File),
                Some("action") if call(1) => (i + 2, Received::Input),
                _ => continue,
            };
            if let Some(at) = handler_param(code, from) {
                o.params.push((code[at].text.clone(), received));
                o.roles.insert(at, Class::Parameter);
            }
        }
        o
    }

    /// `import … from "source"` from `i`, its names recorded: where it ends.
    fn import(&mut self, code: &[&Lexeme], i: usize) -> usize {
        let word = |i: usize| word_of(code.get(i));
        let mut j = i + 1;
        if word(j) == Some("type") {
            return j;
        }
        // Each name: where its local name is, what it imports, and its local name.
        let mut names: Vec<(usize, String, String)> = vec![];
        if let Some(local) = word(j).filter(|w| *w != "from") {
            names.push((j, local.to_string(), local.to_string()));
            j += 1;
            if is_punct(code.get(j), ",") {
                j += 1;
            }
        }
        if is_punct(code.get(j), "{") {
            j += 1;
            while let Some(l) = code.get(j) {
                if is_punct(Some(l), "}") {
                    j += 1;
                    break;
                }
                // (Half typed: no `}` before the source.)
                if l.lex == Lex::Str || (l.lex == Lex::Word && l.text == "from") {
                    break;
                }
                if l.lex == Lex::Word {
                    if word(j + 1) == Some("as")
                        && let Some(local) = word(j + 2)
                    {
                        self.roles.insert(j + 1, Class::Keyword);
                        names.push((j + 2, l.text.clone(), local.to_string()));
                        j += 3;
                        continue;
                    }
                    names.push((j, l.text.clone(), l.text.clone()));
                }
                j += 1;
            }
        }
        let source = match (word(j), code.get(j + 1)) {
            (Some("from"), Some(s)) if s.lex == Lex::Str => s.text.trim_start_matches(['"', '\'']).trim_end_matches(['"', '\'']).to_string(),
            _ => return j.max(i + 1),
        };
        j += 2;
        // `with { optional: "true" }`.
        let mut optional = false;
        if word(j) == Some("with") && is_punct(code.get(j + 1), "{") {
            let close = (j + 1..code.len()).find(|&k| is_punct(code.get(k), "}")).unwrap_or(code.len());
            optional = (j + 2..close).any(|k| word(k) == Some("optional") && code.get(k + 2).is_some_and(|v| v.text.trim_matches(['"', '\'']) == "true"));
            j = close + 1;
        }
        for (at, imported, local) in names {
            let Some(what) = classify(&source, &imported, optional) else { continue };
            let class = match &what {
                Imported::Vocab("stripe") => Class::Noun,
                Imported::Vocab(_) => Class::Keyword,
                Imported::Leaf(_) => Class::Function,
                _ => Class::Definition,
            };
            self.roles.insert(at, class);
            self.imports.push((local, what));
        }
        j
    }

    /// The names a `const`, `let` or `var` binds, its pattern's or its name, from `j`: the flow's
    /// when `flows` (not a `js()` body's).
    fn declaration(&mut self, code: &[&Lexeme], j: usize, flows: bool) {
        let mut names: Vec<String> = vec![];
        match code.get(j) {
            Some(l) if l.lex == Lex::Word => {
                if is_function(code, j + 1) {
                    if flows {
                        self.functions.push(l.text.clone());
                    }
                    self.roles.insert(j, Class::Function);
                } else {
                    names.push(l.text.clone());
                    self.roles.insert(j, Class::Definition);
                }
            }
            Some(l) if is_punct(Some(l), "{") || is_punct(Some(l), "[") => {
                let mut depth = 0;
                for (k, &l) in code.iter().enumerate().skip(j) {
                    if l.lex == Lex::Punct {
                        match l.text.as_str() {
                            "{" | "[" => depth += 1,
                            "}" | "]" => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    } else if l.lex == Lex::Word {
                        // `{ total: sum }`: a key, then its name; `{ n = 1 }`: a default.
                        if is_punct(code.get(k + 1), ":") {
                            self.roles.insert(k, Class::Property);
                        } else if !is_punct(code.get(k - 1), "=") {
                            names.push(l.text.clone());
                            self.roles.insert(k, Class::Definition);
                        }
                    }
                }
            }
            _ => {}
        }
        if flows {
            self.bindings.extend(names);
        }
    }

    /// Whether the text defines `word` itself: a binding, a function, a handler's parameter.
    fn defines(&self, word: &str) -> bool {
        self.bindings.iter().chain(&self.functions).any(|n| n == word) || self.params.iter().any(|(n, _)| n == word)
    }

    /// What `word` is imported as; or, not imported and not defined, what the word is by itself:
    /// a word of the vocabulary, `state`, `now` or `random`, as it would be once imported.
    fn what(&self, word: &str) -> Option<Imported> {
        if let Some((_, what)) = self.imports.iter().find(|(name, _)| name == word) {
            return Some(what.clone());
        }
        if self.defines(word) {
            return None;
        }
        match word {
            "state" => Some(Imported::State),
            "now" => Some(Imported::Now),
            "random" => Some(Imported::Random),
            "random_bytes" => Some(Imported::RandomBytes),
            w => VOCABULARY.iter().find(|v| **v == w).map(|v| Imported::Vocab(v)),
        }
    }

    fn vocab(&self, word: &str) -> Option<&'static str> {
        match self.what(word) {
            Some(Imported::Vocab(v)) => Some(v),
            _ => None,
        }
    }

    fn received(&self, word: &str) -> Option<Received> {
        self.params.iter().find(|(n, _)| n == word).map(|(_, r)| *r)
    }

    /// The vocabulary a call is made with, from its `(`: `flow(`, `every(`, `route.get(` (route),
    /// `on.file.created(` (on).
    fn callee(&self, code: &[&Lexeme], paren: usize) -> Option<&'static str> {
        let word = |i: usize| word_of(code.get(i));
        let i = paren.checked_sub(1)?;
        let name = word(i)?;
        if !i.checked_sub(1).is_some_and(|j| is_dot(code.get(j))) {
            return self.vocab(name);
        }
        let head = word(i.checked_sub(2)?)?;
        if self.vocab(head) == Some("route") {
            return Some("route");
        }
        let on = i.checked_sub(4).and_then(word).and_then(|w| self.vocab(w));
        (head == "file" && on == Some("on")).then_some("on")
    }

    /// The call whose arguments the word at `k` is in, by its vocabulary, if it isn't inside a
    /// function passed to it: `port` in `flow("f", { port: 8080 })` is in `flow`'s.
    fn call_of(&self, code: &[&Lexeme], k: usize) -> Option<&'static str> {
        // Back to the `(` the word is inside, out through the records and lists it is in; an
        // arrow on the way with no `,` after it holds the word in its body.
        let (mut depth, mut comma) = (0usize, false);
        for i in (0..k).rev() {
            let l = code[i];
            match (l.lex, l.text.as_str()) {
                (Lex::Punct, ")" | "]" | "}") | (Lex::Interp, "}") => depth += 1,
                (Lex::Punct, "[" | "{") | (Lex::Interp, "${") if depth > 0 => depth -= 1,
                (Lex::Punct, "[" | "{") | (Lex::Interp, "${") => comma = false,
                (Lex::Punct, "(") if depth > 0 => depth -= 1,
                (Lex::Punct, "(") => return self.callee(code, i),
                (Lex::Punct, ",") if depth == 0 => comma = true,
                (Lex::Punct, "=>") if depth == 0 && !comma => return None,
                _ => {}
            }
        }
        None
    }
}

/// What a name imported from `source` is, `imported` its name there.
fn classify(source: &str, imported: &str, optional: bool) -> Option<Imported> {
    let (kind, rest) = source.split_once(':').unwrap_or((source, ""));
    let declared = |k: &'static str| Some(Imported::Declared(k, rest.to_string()));
    match (kind, rest) {
        ("holon", "") => Some(match VOCABULARY.iter().find(|v| **v == imported) {
            Some(v) => Imported::Vocab(v),
            None => Imported::Leaf(imported.to_string()),
        }),
        ("holon", "clock") => (imported == "now").then_some(Imported::Now),
        ("holon", "random") => match imported {
            "random" => Some(Imported::Random),
            "random_bytes" => Some(Imported::RandomBytes),
            _ => None,
        },
        ("state", "") => Some(Imported::State),
        (_, "") => None,
        ("secret", name) => Some(Imported::Secret { name: name.to_string(), optional }),
        ("collection", _) => declared("collection"),
        ("folder", _) => declared("folder"),
        ("database", _) => declared("database"),
        ("channel", _) => declared("channel"),
        ("mail", _) => declared("mail"),
        ("feed", _) => declared("feed"),
        ("host", host) => Some(Imported::Host(host.to_string())),
        ("flow", slug) => Some(Imported::Action { flow: slug.to_string(), action: imported.to_string() }),
        _ => None,
    }
}

/// Whether what follows `=` at `j` is a function: `(text) => …`, `async (…) => …`, `function …`,
/// `js(…)`.
fn is_function(code: &[&Lexeme], j: usize) -> bool {
    if !is_punct(code.get(j), "=") {
        return false;
    }
    let word = |i: usize| word_of(code.get(i));
    match word(j + 1) {
        Some("async" | "function") => return true,
        Some("js") => return is_punct(code.get(j + 2), "("),
        Some(_) => return is_punct(code.get(j + 2), "=>"),
        None => {}
    }
    if !is_punct(code.get(j + 1), "(") {
        return false;
    }
    // `(…)` closed, then `=>`.
    let mut depth = 0;
    for k in j + 1..code.len() {
        match code[k].text.as_str() {
            "(" if code[k].lex == Lex::Punct => depth += 1,
            ")" if code[k].lex == Lex::Punct => {
                depth -= 1;
                if depth == 0 {
                    return is_punct(code.get(k + 1), "=>") || is_punct(code.get(k + 1), ":");
                }
            }
            _ => {}
        }
    }
    false
}

/// Where a handler's parameter is named, its function starting at `j`: `(req) =>`, `async (f) =>`,
/// `function (input)`, `req =>`; none when it destructures what it receives or leaves it out.
fn handler_param(code: &[&Lexeme], mut j: usize) -> Option<usize> {
    let word = |i: usize| word_of(code.get(i));
    if word(j) == Some("async") {
        j += 1;
    }
    if word(j) == Some("function") {
        j += if word(j + 1).is_some() { 2 } else { 1 };
    }
    if is_punct(code.get(j), "(") && word(j + 1).is_some() && [")", ":", "=", ","].iter().any(|p| is_punct(code.get(j + 2), p)) {
        return Some(j + 1);
    }
    (word(j).is_some() && is_punct(code.get(j + 1), "=>")).then_some(j)
}

/// The text's words, its comments left out: what is read for what the text says.
fn code_of(lexemes: &[Lexeme]) -> Vec<&Lexeme> {
    lexemes.iter().filter(|l| l.lex != Lex::Comment).collect()
}

// ── hover ───────────────────────────────────────────────────────────────────────────────────

/// What the word at `at` is: an imported name (a secret, saying where the flow sends it; the
/// state; a collection, folder, database, channel or host; the clock; another flow's action), a
/// member (`state.x`, `notes.list`, `api.get`, `req.json`, a method that is a leaf), the vocabulary
/// (`flow` says what `holon check` says of the flow), a handler's parameter, a setting or an
/// option, a keyword, a function of this text, a leaf, or a binding (the node it is).
pub fn hover(source: &str, at: Pos) -> Option<Hover> {
    let lexemes = scan(source);
    let code = code_of(&lexemes);
    let k = code.iter().position(|l| l.lex == Lex::Word && covers(l.span, at))?;
    let token = code[k];
    let word = token.text.as_str();
    let outline = Outline::read(&code);
    let said = |text: String| Some(Hover { span: token.span, text });
    // Where this flow sends the secret `name`, as its approval says it; nothing when it sends it
    // nowhere (or the text doesn't compile).
    let sent_to = |name: &str| -> String {
        let sent = commands::sent_of(source);
        match sent.get(name).map(crate::describe::describe_sent_to) {
            Some((Some(own), Some(answers))) => format!(" This flow sends it to {own}; what is answered to it goes to {answers}."),
            Some((Some(own), None)) => format!(" This flow sends it to {own}."),
            Some((None, Some(answers))) => format!(" What is answered to it goes to {answers}."),
            _ => String::new(),
        }
    };

    // A member: `state.x`, `posts.put`, `api.get`, `req.json`; a value's field says nothing.
    if is_dot(k.checked_sub(1).and_then(|j| code.get(j))) {
        return member(&outline, &code, k).and_then(said);
    }
    let what = outline.what(word);
    if what == Some(Imported::Vocab("flow")) {
        let report = commands::check("flow.holon.ts", source);
        let mut lines = report.out;
        lines.extend(report.err);
        return said(format!("```\n{}\n```\nWhat `holon check` says of it, and what deploying it asks a person to approve.", lines.join("\n")));
    }
    match what {
        Some(Imported::Secret { name, optional }) => {
            let which = if optional { "an optional secret" } else { "a secret" };
            return said(format!("`{word}`: {which} of this flow, `secret:{name}`.{} {}", sent_to(&name), noun("secret")));
        }
        Some(Imported::State) => return said(format!("`{word}`: this flow's state. {}", noun("state"))),
        Some(Imported::Declared(kind, name)) => return said(format!("`{word}`: a {kind} of this flow, `{kind}:{name}`. {}", noun(kind))),
        Some(Imported::Host(host)) => return said(format!("`{word}`: the host {host}, called over HTTPS. {}", noun("host"))),
        Some(Imported::Now) => return said(noun("now").into()),
        Some(Imported::Random) => return said(noun("random").into()),
        Some(Imported::RandomBytes) => return said(noun("random_bytes").into()),
        Some(Imported::Action { flow, action }) => return said(format!("`{word}`: the action `{action}` of the deployed flow `{flow}`. {}", noun("flow"))),
        Some(Imported::Vocab("stripe")) => return said(noun("stripe").into()),
        Some(Imported::Vocab(v)) => return said(keyword(v).unwrap_or_default().into()),
        Some(Imported::Leaf(name)) => {
            if let Some(text) = leaf(&name) {
                return said(text);
            }
        }
        None => {}
    }
    if let Some(received) = outline.received(word) {
        return said(format!("`{word}`: {}", received.said().1));
    }
    // A flow's setting or a handler's option: `port:`, `log:`.
    if is_punct(code.get(k + 1), ":")
        && let Some(text) = outline.call_of(&code, k).and_then(|call| option(call, word))
    {
        return said(text.into());
    }
    if !outline.defines(word) {
        if let Some(line) = keyword(word) {
            return said(line.into());
        }
        if matches!(word, "JSON" | "Object") {
            return said(noun(word).into());
        }
    }
    // A function of this text: its signature, and its outputs.
    let names = |ports: &[PortDecl]| ports.iter().map(|p| p.name.text.clone()).collect::<Vec<_>>().join(", ");
    for item in ts::parse(source).iter().flat_map(|f| &f.items) {
        match item {
            Item::Holon(h) if h.name.text == word => {
                return said(fenced(&format!("function {word}({})", names(&h.inputs)), &format!("A function of this flow, defined on line {}: {}.", h.name.span.start.line, outputs(&h.outputs))));
            }
            Item::Js(j) if j.name.text == word => {
                let defined = format!("A js() function of this flow, defined on line {}, run as JavaScript: {}.", j.name.span.start.line, outputs(&j.outputs));
                return said(fenced(&format!("const {word} = js(function ({}) {{ … }})", names(&j.inputs)), &defined));
            }
            _ => {}
        }
    }
    if !outline.defines(word)
        && let Some(text) = leaf(word)
    {
        return said(text);
    }
    // A binding: the node it is.
    let compiled = ts::compile(source).ok()?;
    let id = compiled.names.iter().find(|(_, name)| name.as_str() == word).map(|(id, _)| id)?;
    let node = compiled.doc.nodes.iter().find(|n| &n.id == id)?;
    let what = leaf(&node.data.name).map(|l| format!("\n\n{l}")).unwrap_or_default();
    said(format!("`{word}`: a `{}` node.{what}", node.data.name))
}

/// What `head.word` is, the word at `k` after a `.`: a part of a route's request, a method of what
/// the head is imported as, a key of the state, a method that is a leaf; a value's field, nothing.
fn member(o: &Outline, code: &[&Lexeme], k: usize) -> Option<String> {
    let word = code[k].text.as_str();
    if let Some(head) = k.checked_sub(2).and_then(|h| word_of(code.get(h))) {
        let named = format!("`{head}.{word}`");
        match o.what(head) {
            Some(Imported::State) => return Some(format!("{named}: a key of this flow's state. {}", noun("state"))),
            Some(Imported::Declared(kind, name)) => return Some(format!("{named}: on the {kind} `{name}`. {}", noun(kind))),
            Some(Imported::Host(host)) => return Some(format!("{named}: a request to {host}. {}", noun("host"))),
            Some(Imported::Vocab("stripe")) => return Some(noun("stripe").into()),
            Some(Imported::Vocab(v @ ("route" | "on"))) => return keyword(v).map(str::to_string),
            _ => {}
        }
        if o.received(head) == Some(Received::Request) {
            return Some(format!("{named}: part of {REQUEST}"));
        }
        // `on.file.created`: the head's head.
        let on = k.checked_sub(4).and_then(|h| word_of(code.get(h))).and_then(|w| o.vocab(w));
        if head == "file" && on == Some("on") {
            return keyword("on").map(str::to_string);
        }
        if !o.defines(head) {
            match (head, word) {
                ("JSON", "parse") => return leaf("json_parse"),
                ("JSON", "stringify") => return leaf("json_stringify"),
                ("Object", "keys") => return leaf("keys"),
                ("Object", "values") => return leaf("values"),
                ("Object", "entries") => return leaf("entries"),
                ("Object", "fromEntries") => return leaf("from_entries"),
                ("Object", "groupBy") => return leaf("group_by"),
                ("Object", "assign") => return leaf("merge"),
                ("Object", "hasOwn") => return leaf("has"),
                ("Object", "is") => return leaf("same_value"),
                ("Date", "UTC") => return leaf("date_utc"),
                ("Date", "parse") => return leaf("date_parse"),
                ("Array", "isArray") => return leaf("is_list"),
                ("String", "fromCharCode" | "fromCodePoint") => return leaf("from_codes"),
                _ => {}
            }
        }
    }
    // A method that is a leaf: `.toUpperCase()` is `uppercase`.
    let called = is_punct(code.get(k + 1), "(");
    let (_, does) = METHOD_LEAVES.iter().find(|(m, _)| *m == word).filter(|_| called)?;
    Some(format!("`.{word}(…)` is the leaf `{does}`.\n\n{}", leaf(does)?))
}

// ── completion ──────────────────────────────────────────────────────────────────────────────

/// What a completion offers is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// TypeScript's keywords and Holon's vocabulary.
    Keyword,
    /// A leaf, a function of this flow, or another flow's action: called as `name(…)`.
    Function,
    /// A binding, a handler's parameter, a secret.
    Variable,
    /// What the flow imports from outside a run: the state, a collection, a host, the clock.
    Module,
    /// After a `.`: a part of the request, a method of what is imported, a key of the state.
    Member,
}

/// One word a completion offers, with what it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub label: String,
    pub kind: Kind,
    /// A line: a signature, or what it is.
    pub detail: String,
    /// Markdown, if there is more to say.
    pub doc: Option<String>,
}

/// Leaves a text doesn't call by name: it writes them as what it imports (`api.get`, `state.x`,
/// `notes[k]`, `shop.query(…)`, `now`, a secret, an action called), as `await event(…)`,
/// `stripe.verify`, `JSON.parse`, or literals (`1`, `{ … }`).
const SPOKEN_OTHERWISE: &[&str] = &[
    "http_request", "http_upload", "http_stream", "clock", "random", "random_bytes", "file_arrival", "secret", "await", "store_read", "store_write", "collection_get", "collection_list", "collection_put",
    "collection_delete", "file_read", "file_list", "file_write", "file_delete", "file_handle", "file_lines", "file_slice", "file_copy", "file_create", "file_create_copy", "db_query", "db_one",
    "db_exec", "db_find", "db_find_one", "db_insert", "db_update", "db_delete", "egress", "channel_send", "channel_subscribe", "mail_send", "record", "json_parse", "json_stringify", "flow_start", "flow_call",
    "stripe_verify",
];

/// The leaves a text calls by name, imported from `"holon"`: their name, signature and what they
/// do. (One named as a keyword, `const` or `default`, can't be imported: it is written otherwise.)
fn callable() -> Vec<(String, String, String)> {
    let named = |n: &str| !SPOKEN_OTHERWISE.contains(&n) && !TS_KEYWORDS.contains(&n);
    agent::leaves().into_iter().filter(|(n, _, _)| named(n)).map(|(n, s, d)| (n, s, d.to_string())).collect()
}

fn word(label: &str, kind: Kind, detail: impl Into<String>, doc: Option<String>) -> Completion {
    Completion { label: label.to_string(), kind, detail: detail.into(), doc }
}

/// Methods, each with what it does, offered after `head.`.
fn methods(head: &str, all: &[(&str, &str)]) -> Vec<Completion> {
    all.iter().map(|(m, d)| word(m, Kind::Member, format!("{head}.{d}"), None)).collect()
}

const ROUTE: &[(&str, &str)] = &[
    ("get", "get(\"/path\", (req) => …)"),
    ("post", "post(\"/path\", (req) => …)"),
    ("put", "put(\"/path\", (req) => …)"),
    ("patch", "patch(\"/path\", (req) => …)"),
    ("delete", "delete(\"/path\", (req) => …)"),
];
const COLLECTION: &[(&str, &str)] = &[
    ("get", "get(key): the item, or no value"),
    ("put", "put(key, value): written when the run finishes"),
    ("delete", "delete(key): removed when the run finishes"),
    ("list", "list({ prefix: p, after: a, limit: 50 }): a page of items"),
];
const FOLDER: &[(&str, &str)] = &[
    ("get", "get(name): the file's text, or no value"),
    ("put", "put(name, text): written when the run finishes"),
    ("delete", "delete(name): deleted when the run finishes"),
    ("list", "list({ prefix: p, after: a, limit: 50 }): a page of files, each {size, hash, modified}"),
    ("file", "file(name): a handle to the file, {folder, name, size, hash, modified}, no bytes"),
    ("lines", "lines(name, { from: i, limit: n }): n lines from line i (from the end when negative)"),
    ("slice", "slice(name, { offset: o, length: n }): n bytes from offset o, as text"),
    ("copy", "copy(name, handle): the file copied by its handle when the run finishes"),
    ("create", "create(name, text): written when the run finishes, only if there is no such file yet"),
    ("create_copy", "create_copy(name, handle): copied when the run finishes, only if there is no such file yet"),
];
const DATABASE: &[(&str, &str)] = &[
    ("query", "query(\"select …\", v1, …): the rows, a list of records, as the run began"),
    ("one", "one(\"select …\", v1, …): the first row, or no value"),
    ("exec", "exec(\"update …\", v1, …, { expect: n }): run when the run finishes"),
    ("find", "find(\"coll\", { … }, { sort: { … }, limit: n }): the documents the filter selects"),
    ("find_one", "find_one(\"coll\", { … }): the first document, or no value"),
    ("insert", "insert(\"coll\", { … }): inserted when the run finishes"),
    ("update", "update(\"coll\", { … }, { \"$set\": { … } }, { expect: n }): updated when the run finishes"),
    ("delete", "delete(\"coll\", { … }): deleted when the run finishes"),
];
const CHANNEL: &[(&str, &str)] = &[
    ("send", "send(pack): sent to its subscribers once the run is over"),
    ("pipe", "pipe(await api.post(…)): the call's answer, to its subscribers as it arrives"),
    ("subscribe", "subscribe({ from: \"kept\" }): a route's answer, the packs as server-sent events"),
];
const MAIL: &[(&str, &str)] = &[("send", "send({ to, cc, subject, text, html, attachments }): mailed once the run is over")];
const HOST: &[&str] = &["get", "post", "put", "patch", "delete", "head"];
const REQUEST_PARTS: &[(&str, &str)] = &[
    ("params", "the path's :names"),
    ("query", "the query string's names"),
    ("headers", "the request's headers, by lowercase name"),
    ("body", "the body, as text"),
    ("json", "the body, parsed as JSON"),
];

/// What may be written at `at`: after `head.`, `head`'s members; elsewhere, this text's
/// functions, bindings, handlers' parameters and imported names, Holon's vocabulary, the leaves it
/// calls by name, and TypeScript's keywords. Inside a string or a comment, nothing.
pub fn complete(source: &str, at: Pos) -> Vec<Completion> {
    let lexemes = scan(source);
    let inside = |l: &Lexeme| before(l.span.start, at) && (before(at, l.span.end) || (at == l.span.end && l.open));
    if lexemes.iter().any(|l| matches!(l.lex, Lex::Str | Lex::Escape | Lex::Comment) && inside(l)) {
        return vec![];
    }
    let code = code_of(&lexemes);
    let outline = Outline::read(&code);
    // The word being typed: the one the cursor is in or just after, as far as the cursor.
    let typed = code.iter().position(|l| l.lex == Lex::Word && l.span.start.line == at.line && before(l.span.start, at) && !before(l.span.end, at));
    let partial: String = typed.map(|k| code[k].text.chars().take((at.col - code[k].span.start.col) as usize).collect()).unwrap_or_default();
    // (Names may be used before the cursor and defined after it: the whole text is read.)
    let here = typed.unwrap_or_else(|| code.iter().position(|l| !before(l.span.start, at)).unwrap_or(code.len()));
    let offered = if here.checked_sub(1).is_some_and(|i| is_dot(code.get(i))) {
        let Some(head) = here.checked_sub(2).filter(|&h| code[h].lex == Lex::Word) else { return vec![] };
        members(&outline, &code, head, at)
    } else {
        everything(&outline)
    };
    let mut seen = BTreeSet::new();
    offered.into_iter().filter(|c| c.label.starts_with(partial.as_str()) && seen.insert(c.label.clone())).collect()
}

/// The members of the word at `head`, by what it is: a route's request, what it is imported as,
/// `route`, `on` and `on.file`, `JSON`, `Object`; a value's fields aren't known.
fn members(o: &Outline, code: &[&Lexeme], head: usize, at: Pos) -> Vec<Completion> {
    let h = code[head].text.as_str();
    match o.what(h) {
        // The keys the text uses (not the one being typed).
        Some(Imported::State) => {
            let mut keys: Vec<String> = (0..code.len())
                .filter(|&i| word_of(code.get(i)) == Some(h) && is_dot(code.get(i + 1)))
                .filter_map(|i| code.get(i + 2).filter(|k| k.lex == Lex::Word && k.span.end != at).map(|k| k.text.clone()))
                .collect();
            keys.sort();
            keys.dedup();
            keys.iter().map(|k| word(k, Kind::Variable, format!("{h}.{k}: a key of this flow's state"), None)).collect()
        }
        Some(Imported::Declared(kind, _)) => methods(
            h,
            match kind {
                "collection" => COLLECTION,
                "folder" => FOLDER,
                "database" => DATABASE,
                "mail" => MAIL,
                "feed" => &[],
                _ => CHANNEL,
            },
        ),
        Some(Imported::Host(host)) => HOST.iter().map(|m| word(m, Kind::Member, format!("{h}.{m}(path, {{ headers: h, body: b }}): a request to {host}"), Some(noun("host").into()))).collect(),
        Some(Imported::Vocab("route")) => methods(h, ROUTE),
        Some(Imported::Vocab("on")) => vec![
            word("file", Kind::Member, format!("{h}.file.created(folder, (f) => …), {h}.file.deleted(…)"), keyword("on").map(str::to_string)),
            word("mail", Kind::Member, format!("{h}.mail(mail, (m) => …)"), keyword("on").map(str::to_string)),
            word("feed", Kind::Member, format!("{h}.feed(feed, (e) => …)"), keyword("on").map(str::to_string)),
        ],
        Some(Imported::Vocab("stripe")) => vec![word("verify", Kind::Member, format!("{h}.verify(req, secret, now)"), Some(noun("stripe").into()))],
        _ if o.received(h) == Some(Received::Request) => REQUEST_PARTS.iter().map(|(m, d)| word(m, Kind::Member, format!("{h}.{m}: {d}"), None)).collect(),
        _ if h == "file" && head.checked_sub(2).and_then(|i| word_of(code.get(i))).and_then(|w| o.vocab(w)) == Some("on") => {
            methods("on.file", &[("created", "created(folder, (f) => …): each file new or changed"), ("deleted", "deleted(folder, (f) => …): each file removed")])
        }
        _ if o.defines(h) => vec![],
        _ if h == "JSON" => methods(h, &[("parse", "parse(text): the leaf json_parse"), ("stringify", "stringify(x): the leaf json_stringify")]),
        _ if h == "Object" => methods(h, &[("keys", "keys(r): the leaf keys")]),
        _ => vec![],
    }
}

/// Everything a text may write where a name starts: its own names first, then what is offered to
/// every text.
fn everything(o: &Outline) -> Vec<Completion> {
    let mut v: Vec<Completion> = vec![];
    v.extend(o.functions.iter().map(|f| word(f, Kind::Function, "a function of this flow", None)));
    v.extend(o.bindings.iter().map(|b| word(b, Kind::Variable, "a binding of this flow", None)));
    v.extend(o.params.iter().map(|(p, r)| word(p, Kind::Variable, r.said().0, Some(r.said().1.to_string()))));
    for (name, what) in &o.imports {
        let (kind, detail, doc) = match what {
            Imported::Secret { name: s, .. } => (Kind::Variable, format!("a secret of this flow, secret:{s}"), noun("secret")),
            Imported::State => (Kind::Module, "this flow's state".into(), noun("state")),
            Imported::Declared(k, n) => (Kind::Module, format!("a {k} of this flow, {k}:{n}"), noun(k)),
            Imported::Host(host) => (Kind::Module, format!("the host {host}"), noun("host")),
            Imported::Now => (Kind::Module, "the clock: one permission".into(), noun("now")),
            Imported::Random => (Kind::Module, "randomness: one permission".into(), noun("random")),
            Imported::RandomBytes => (Kind::Module, "the run's random bytes: one permission".into(), noun("random_bytes")),
            Imported::Action { flow, action } => (Kind::Function, format!("the action {action} of the flow {flow}"), noun("flow")),
            // Offered as the vocabulary and the leaves are, below.
            Imported::Vocab(_) | Imported::Leaf(_) => continue,
        };
        v.push(word(name, kind, detail, Some(doc.to_string())));
    }
    for w in VOCABULARY {
        let doc = if *w == "stripe" { Some(noun(w)) } else { keyword(w) };
        v.push(word(w, if *w == "stripe" { Kind::Module } else { Kind::Keyword }, "Holon's vocabulary, imported from \"holon\"", doc.map(str::to_string)));
    }
    v.extend(["JSON", "Object"].iter().map(|g| word(g, Kind::Module, "their methods are leaves", Some(noun(g).to_string()))));
    v.extend(callable().into_iter().map(|(n, sig, doc)| word(&n, Kind::Function, sig, (!doc.is_empty()).then_some(doc))));
    v.extend(KEYWORDS.iter().chain(SAID_WITH).map(|(k, _)| word(k, Kind::Keyword, "keyword", keyword(k).map(str::to_string))));
    v.extend(["true", "false", "null"].iter().map(|k| word(k, Kind::Keyword, "keyword", None)));
    v
}

// ── colours ─────────────────────────────────────────────────────────────────────────────────

/// What a stretch of a text is, for colouring it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Comment,
    /// TypeScript's keywords and Holon's vocabulary (`flow`, `route`, `status`, …).
    Keyword,
    /// What the flow imports from outside a run, where it is used: a secret, the state, a
    /// collection, a host, `now`, another flow's action, `stripe`.
    Noun,
    /// A call, a function's name, a leaf imported.
    Function,
    /// What a statement or an import names: a binding, an imported name, the flow.
    Definition,
    /// A handler's parameter: a route's request, a file's handle, an action's input, a tick.
    Parameter,
    /// After a `.`.
    Member,
    /// A record's key: `status:`, a setting, an option.
    Property,
    /// A route's HTTP method: `route.post`.
    Method,
    /// A route's path.
    Path,
    String,
    Escape,
    /// A template's `${` and its `}` (what is inside is coloured as code).
    Interpolation,
    Number,
    Boolean,
    Operator,
}

/// One coloured stretch: within one line.
#[derive(Clone, Debug, PartialEq)]
pub struct Highlight {
    pub span: Span,
    pub class: Class,
}

/// TypeScript's reserved words, and `undefined`, coloured as keywords wherever they are.
const TS_KEYWORDS: &[&str] = &[
    "import", "export", "const", "let", "var", "function", "async", "await", "return", "if", "else", "switch", "case", "default", "delete", "typeof", "instanceof", "new", "in", "void", "with",
    "for", "while", "do", "break", "continue", "try", "catch", "finally", "throw", "class", "extends", "this", "super", "yield", "null", "undefined", "enum",
];

/// The class of the word at `k` in the code's words; `inner`, the bracket each is inside.
fn word_class(o: &Outline, code: &[&Lexeme], inner: &[Option<char>], k: usize) -> Option<Class> {
    if let Some(&class) = o.roles.get(&k) {
        return Some(class);
    }
    let w = code[k].text.as_str();
    let p = k.checked_sub(1).and_then(|i| code.get(i));
    let n = code.get(k + 1);
    if is_dot(p) {
        let route = k.checked_sub(2).and_then(|h| word_of(code.get(h))).and_then(|h| o.vocab(h)) == Some("route");
        return Some(if route && ROUTE.iter().any(|(m, _)| *m == w) { Class::Method } else { Class::Member });
    }
    let word_next = n.is_some_and(|n| n.lex == Lex::Word);
    let keyword = match w {
        "from" => n.is_some_and(|n| n.lex == Lex::Str),
        "type" | "interface" | "declare" => word_next,
        "as" | "of" => word_next && p.is_some_and(|p| p.lex == Lex::Word || is_punct(Some(p), ")") || is_punct(Some(p), "]")),
        w => TS_KEYWORDS.contains(&w),
    };
    if keyword {
        return Some(Class::Keyword);
    }
    if w == "true" || w == "false" {
        return Some(Class::Boolean);
    }
    // `{ status: 201 }`: a record's key, whatever its word (not a choice's `c ? a : b`).
    if is_punct(n, ":") && inner[k] == Some('{') && (is_punct(p, "{") || is_punct(p, ",")) {
        return Some(Class::Property);
    }
    match o.what(w) {
        Some(Imported::Vocab("stripe")) => return Some(Class::Noun),
        Some(Imported::Vocab(_)) => return Some(Class::Keyword),
        Some(Imported::Leaf(_)) | None => {}
        Some(_) => return Some(Class::Noun),
    }
    is_punct(n, "(").then_some(Class::Function)
}

/// The class of the code's word at `k`, of whatever kind.
fn class_of(o: &Outline, code: &[&Lexeme], inner: &[Option<char>], k: usize) -> Option<Class> {
    let l = code[k];
    match l.lex {
        Lex::Word => word_class(o, code, inner, k),
        Lex::Number => Some(Class::Number),
        Lex::Escape => Some(Class::Escape),
        Lex::Interp => Some(Class::Interpolation),
        Lex::Comment => Some(Class::Comment),
        Lex::Punct => OPERATORS.contains(&l.text.as_str()).then_some(Class::Operator),
        // A call's first argument: the flow's name, a route's path.
        Lex::Str if k > 0 && is_punct(code.get(k - 1), "(") => match o.callee(code, k - 1) {
            Some("flow") => Some(Class::Definition),
            Some("route") => Some(Class::Path),
            _ => Some(Class::String),
        },
        Lex::Str => Some(Class::String),
    }
}

/// The bracket each of the code's words is inside: `(`, `[`, `{`, or `$` for a template's `${…}`.
fn brackets(code: &[&Lexeme]) -> Vec<Option<char>> {
    let mut open: Vec<char> = vec![];
    let mut inner = Vec::with_capacity(code.len());
    for l in code {
        inner.push(open.last().copied());
        match (l.lex, l.text.as_str()) {
            (Lex::Punct, b @ ("(" | "[" | "{")) => open.push(b.chars().next().unwrap_or('(')),
            (Lex::Interp, "${") => open.push('$'),
            (Lex::Punct, ")" | "]" | "}") | (Lex::Interp, "}") => {
                open.pop();
            }
            _ => {}
        }
    }
    inner
}

/// `span` coloured `class`, a stretch for each line it is on; `widths`, each line's length.
fn mark(out: &mut Vec<Highlight>, widths: &[u32], span: Span, class: Class) {
    for line in span.start.line..=span.end.line {
        let start = if line == span.start.line { span.start.col } else { 1 };
        let end = if line == span.end.line { span.end.col } else { widths.get(line as usize - 1).map_or(start, |w| w + 1) };
        if end > start {
            out.push(Highlight { span: Span { start: Pos { line, col: start }, end: Pos { line, col: end } }, class });
        }
    }
}

/// The text's colours, as TypeScript and Holon read its words: what it imports, defines and hands
/// its handlers coloured as what it is. It never stops: a string not yet closed, a comment not
/// ended, or a line half typed is coloured as far as it goes; a comment or a template over several
/// lines is a stretch on each.
pub fn highlight(source: &str) -> Vec<Highlight> {
    let lexemes = scan(source);
    let code = code_of(&lexemes);
    let outline = Outline::read(&code);
    let inner = brackets(&code);
    let widths: Vec<u32> = source.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).chars().count() as u32).collect();
    let mut out = vec![];
    let mut k = 0;
    for l in &lexemes {
        let class = if l.lex == Lex::Comment {
            Some(Class::Comment)
        } else {
            k += 1;
            class_of(&outline, &code, &inner, k - 1)
        };
        if let Some(class) = class {
            mark(&mut out, &widths, l.span, class);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::samples;

    const SLUGS: &str = include_str!("../samples/slugs.holon.ts");
    const STRIPE: &str = include_str!("../samples/stripe-to-slack.holon.ts");
    const LICENSE: &str = include_str!("../samples/license-check.holon.ts");

    /// Two routes that clash: it compiles, and is wrong as a whole.
    const CLASH: &str = "// two of one route\nimport { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.get(\"/a\", () => \"one\")\nroute.get(\"/a\", () => \"two\")\n";
    const CLASHES: &str = "GET /a clashes with GET /a";
    /// A text, and as fmt writes it.
    const MESSY: &str = "import { flow, route } from \"holon\"\nflow(\"f\", { port: 8200 })\nroute.get(\"/a\",   ()   =>   \"ok\")";
    const TIDY: &str = "import { flow, route } from \"holon\"\nflow(\"f\", { port: 8200 })\nroute.get(\"/a\", () => \"ok\")\n";

    /// The position of the `nth` (from 0) occurrence of `needle` in `source`'s code (a `//`
    /// comment line isn't searched), plus `into` chars.
    fn at(source: &str, needle: &str, nth: usize, into: u32) -> Pos {
        let mut seen = 0;
        for (l, line) in source.lines().enumerate().filter(|(_, line)| !line.trim_start().starts_with("//")) {
            let mut from = 0;
            while let Some(c) = line[from..].find(needle) {
                let col = from + c;
                if seen == nth {
                    return Pos { line: l as u32 + 1, col: line[..col].chars().count() as u32 + 1 + into };
                }
                seen += 1;
                from = col + needle.len();
            }
        }
        panic!("{needle} #{nth} isn't in the text")
    }

    fn says(source: &str, needle: &str, nth: usize) -> String {
        hover(source, at(source, needle, nth, 0)).map(|h| h.text).unwrap_or_default()
    }

    #[test]
    fn every_sample_has_no_mistakes() {
        for s in samples::samples() {
            assert_eq!(mistakes(s.text), vec![], "{}", s.name);
        }
    }

    #[test]
    fn a_mistake_has_its_span_and_the_words_check_uses() {
        let m = mistakes("import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.get(\"/a\", () => nope)\n");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].message, "unknown name \"nope\"");
        assert_eq!((m[0].span.start, m[0].span.end), (Pos { line: 5, col: 23 }, Pos { line: 5, col: 27 }));
        // Compiled, but what it derives to is wrong: said at the `flow(…)` call.
        let m = mistakes(CLASH);
        assert_eq!(m.iter().map(|m| m.message.as_str()).collect::<Vec<_>>(), [CLASHES], "once");
        assert_eq!((m[0].span.start, m[0].span.end), (Pos { line: 4, col: 1 }, Pos { line: 4, col: 26 }), "at the flow(…) call");
        // No flow(…) at all: said at the start.
        let m = mistakes("import { flow } from \"holon\"\n");
        assert_eq!((m.len(), m[0].span), (1, START), "{m:?}");
    }

    #[test]
    fn the_word_under_the_cursor_is_said() {
        assert!(says(SLUGS, "hmac_sha256(", 0).contains("hmac_sha256("), "a leaf, by its signature");
        assert!(says(SLUGS, "hmac_sha256,", 0).contains("hmac_sha256("), "a leaf, where it is imported");
        let lower = hover(SLUGS, at(SLUGS, "toLowerCase", 0, 1)).unwrap().text;
        assert!(lower.contains("the leaf `lowercase`") && lower.contains("lowercase("), "a method that is a leaf: {lower}");
        let secret = says(SLUGS, "receipt_key from", 0);
        assert!(secret.contains("an optional secret of this flow, `secret:receipt_key`") && secret.contains("set by an admin"), "an imported secret: {secret}");
        assert_eq!(says(SLUGS, "receipt_key, slug", 0), secret, "where it is used too");
        assert!(hover(SLUGS, at(SLUGS, "posts.put", 0, 6)).unwrap().text.contains("on the collection `posts`"), "a collection's method");
        assert!(says(SLUGS, "posts.put", 0).contains("a collection of this flow, `collection:posts`"), "a collection");
        assert!(hover(SLUGS, at(SLUGS, "state.last_post", 0, 6)).unwrap().text.contains("a key of this flow's state"), "a state key");
        assert!(says(SLUGS, "state.last_post", 0).contains("as the run began"), "the state");
        assert!(says(SLUGS, "now }", 0).contains("reading the clock"), "the clock");
        assert!(says(SLUGS, "route.post", 0).contains("answers requests on the flow's port"), "the vocabulary");
        assert!(hover(SLUGS, at(SLUGS, "route.post", 0, 6)).unwrap().text.contains("answers requests"), "a route's method");
        assert!(says(SLUGS, "status(400)", 0).contains("a route's answer with that status"), "the vocabulary, called");
        assert!(says(SLUGS, "const title", 0).contains("binds a name"), "a keyword");
        assert!(says(SLUGS, "req) =>", 0).contains(".params"), "the request");
        assert!(hover(SLUGS, at(SLUGS, "req.json", 0, 4)).unwrap().text.contains(".json"), "a part of it");
        assert!(says(SLUGS, "port:", 0).contains("the port the flow's server answers"), "a flow's setting");
        let defined = at(SLUGS, "function slugify", 0, 0).line;
        assert_eq!(says(SLUGS, "slugify(req", 0), format!("```ts\nfunction slugify(text)\n```\nA function of this flow, defined on line {defined}: its output is `slug`."));
        assert!(says(SLUGS, "publish(title", 0).contains("its outputs are `put` and `receipt`"), "a function with two outputs");
        assert!(says(SLUGS, "published =", 0).contains("`published`: a `publish` node"), "a binding");
        let flow = says(SLUGS, "flow(", 0);
        assert!(flow.contains("POST /slugs") && flow.contains("it may:") && flow.contains("receive requests"), "the flow: {flow}");
        // Where a secret goes, where it is imported and where it is used; one only verified with
        // goes nowhere.
        let sent = "This flow sends it to https://hooks.slack.com.";
        assert!(says(STRIPE, "slack_webhook_path from", 0).contains(sent), "an imported secret, sent");
        assert!(says(STRIPE, "slack_webhook_path, {", 0).contains(sent), "a secret used, sent");
        let verified = says(STRIPE, "stripe_webhook_secret, now", 0);
        assert!(verified.contains("a secret of this flow") && !verified.contains("sends it"), "{verified}");
        assert!(says(STRIPE, "hooks.post", 0).contains("the host hooks.slack.com"), "a host");
        assert!(hover(STRIPE, at(STRIPE, "hooks.post", 0, 6)).unwrap().text.contains("a request to hooks.slack.com"), "a host's method");
        assert!(says(STRIPE, "stripe.verify", 0).contains("a verified Stripe webhook"), "stripe");
        assert!(says(STRIPE, "JSON.stringify", 0).contains("JSON in and out"), "JSON");
        assert!(hover(STRIPE, at(STRIPE, "JSON.stringify", 0, 5)).unwrap().text.contains("json_stringify("), "its leaf");
        assert!(says(LICENSE, "license_key)", 0).contains("This flow sends it to the database \"vendor\"."), "a query's parameter");
        assert!(says(LICENSE, "vendor.one", 0).contains("a database of this flow, `database:vendor`"), "a database");
        assert_eq!(hover(SLUGS, at(SLUGS, "\"-\"", 0, 1)), None, "inside a string");
        assert_eq!(hover(SLUGS, at(SLUGS, "published.receipt", 0, 10)), None, "a field of a value");
        // A handler's parameter, another flow's action, an option.
        let handlers = "import { action, flow, on, route } from \"holon\"\nimport { charge } from \"flow:billing\"\nimport inbox from \"folder:inbox\"\n\nflow(\"h\", { port: 8200 })\n\non.file.created(inbox, (f) => {\n  inbox[f.name] = \"seen\"\n}, { matching: \"*.csv\" })\n\nexport const pay = action(async (input) => {\n  const paid = await charge(input)\n  return paid.answer\n})\n";
        assert!(says(handlers, "f)", 0).contains("the handle of the file"), "a file's handle");
        assert!(says(handlers, "input)", 0).contains("what the action was sent"), "an action's input");
        assert!(says(handlers, "charge(input", 0).contains("the action `charge` of the deployed flow `billing`"), "another flow's action");
        assert!(says(handlers, "matching", 0).contains("only the files whose names match"), "a handler's option");
        let answered = "import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.get(\"/a\", () => {\n  return { log: 1 }\n}, { log: \"lazy\" })\n";
        assert!(!says(answered, "log: 1", 0).contains("how the handler's runs are recorded"), "a key of what it answers isn't an option");
        assert!(says(answered, "log: \"", 0).contains("how the handler's runs are recorded"), "its option is");
        assert!(hover(handlers, at(handlers, "on.file.created", 0, 8)).unwrap().text.contains("a run for each file"), "on.file.created");
    }

    /// Each coloured stretch of `text` as `(its text, its class)`.
    fn coloured(text: &str) -> Vec<(String, Class)> {
        let lines: Vec<Vec<char>> = text.split('\n').map(|l| l.chars().collect()).collect();
        highlight(text)
            .into_iter()
            .map(|h| {
                assert_eq!(h.span.start.line, h.span.end.line, "within a line");
                let line = &lines[h.span.start.line as usize - 1];
                (line[h.span.start.col as usize - 1..h.span.end.col as usize - 1].iter().collect(), h.class)
            })
            .collect()
    }

    #[test]
    fn a_text_is_coloured_as_the_language_reads_it() {
        use Class::*;
        let has = |text: &str, word: &str, class: Class| {
            let c = coloured(text);
            assert!(c.contains(&(word.to_string(), class)), "{word} as {class:?} in {c:?}");
        };
        let text = "// hi\nimport { flow, lowercase, respond, route } from \"holon\"\nimport notes from \"collection:notes\"\nimport { now } from \"holon:clock\"\n\nflow(\"hello-api\", { port: 8200 })\n\nroute.get(\"/hello/:name\", (req) => {\n  const greeting = `Hi, ${req.params.name}\\n`\n  notes[req.params.name] = now\n  return respond(lowercase(greeting), { status: 201 })\n})\n";
        has(text, "// hi", Comment);
        has(text, "import", Keyword);
        has(text, "from", Keyword);
        has(text, "\"holon\"", String);
        has(text, "flow", Keyword);
        has(text, "\"hello-api\"", Definition);
        has(text, "port", Property);
        has(text, "8200", Number);
        has(text, "notes", Definition);
        has(text, "notes", Noun);
        has(text, "route", Keyword);
        has(text, "get", Method);
        has(text, "\"/hello/:name\"", Path);
        has(text, "req", Parameter);
        has(text, "=>", Operator);
        has(text, "const", Keyword);
        has(text, "greeting", Definition);
        has(text, "`Hi, ", String);
        has(text, "${", Interpolation);
        has(text, "params", Member);
        has(text, "\\n", Escape);
        has(text, "now", Noun);
        has(text, "return", Keyword);
        has(text, "respond", Keyword);
        has(text, "lowercase", Function);
        has(text, "status", Property);
        has(text, "=", Operator);
        // TypeScript's words, and the names a pattern binds.
        let words = "if (x === undefined) { let a = 'one' } else { switch (y) { case 1: delete z[1] } }\nconst { total, n: count } = async () => true\n";
        for w in ["if", "undefined", "let", "else", "switch", "case", "delete", "const", "async"] {
            has(words, w, Keyword);
        }
        has(words, "'one'", String);
        has(words, "true", Boolean);
        has(words, "total", Definition);
        has(words, "n", Property);
        has(words, "count", Definition);
        // Each handler's parameter; an action's name, a function's.
        let handlers = "every(\"1h\", (now) => 1)\non.file.created(inbox, async (f) => 1)\nexport const forget = action((input) => 1)\nfunction slugify(text) {}\n";
        has(handlers, "now", Parameter);
        has(handlers, "f", Parameter);
        has(handlers, "input", Parameter);
        has(handlers, "forget", Definition);
        has(handlers, "slugify", Function);
        // A comment over lines is a stretch on each; a regular expression is a string, and what
        // follows it is read.
        let c = coloured("/* one\ntwo */ x.replace(/[\"']/g, \"\")\n");
        assert!(c.contains(&("/* one".to_string(), Comment)) && c.contains(&("two */".to_string(), Comment)), "{c:?}");
        assert!(c.contains(&("/[\"']/g".to_string(), String)) && c.contains(&("\"\"".to_string(), String)), "{c:?}");
        // Half typed: a string not closed is a string to the line's end, and what follows is read.
        let c = coloured("const x = \"not closed\nconst y = lowercase(x)\n");
        assert!(c.contains(&("\"not closed".to_string(), String)), "{c:?}");
        assert!(c.contains(&("lowercase".to_string(), Function)), "{c:?}");
        // Every sample: in order, never overlapping.
        for s in samples::samples() {
            let h = highlight(s.text);
            for w in h.windows(2) {
                assert!((w[0].span.end.line, w[0].span.end.col) <= (w[1].span.start.line, w[1].span.start.col), "{}: {:?}", s.name, w);
            }
        }
    }

    /// The labels offered with the cursor where `|` is in `text` (the `|` taken out).
    fn offered(text: &str) -> Vec<String> {
        let (l, line) = text.lines().enumerate().find(|(_, l)| l.contains('|')).unwrap();
        let at = Pos { line: l as u32 + 1, col: line[..line.find('|').unwrap()].chars().count() as u32 + 1 };
        complete(&text.replacen('|', "", 1), at).into_iter().map(|c| c.label).collect()
    }

    #[test]
    fn completion_offers_what_may_be_written_there() {
        let body = |line: &str| SLUGS.replacen("  return { title, author, receipt: published.receipt } ?? status(400)", line, 1);
        // A leaf, by the letters typed; with its signature.
        assert_eq!(offered(&body("  return lowe|")), ["lowercase"]);
        let typed = body("  return lowe");
        let lower = complete(&typed, Pos { line: typed.lines().position(|l| l.ends_with("lowe")).unwrap() as u32 + 1, col: 14 });
        assert_eq!((lower[0].kind, lower[0].detail.as_str()), (Kind::Function, "lowercase(s: Text) -> out: Text"));
        // This text's own words: its functions, bindings, imports, the request; the vocabulary,
        // the leaves and the keywords.
        let words = offered(&body("  return |"));
        for w in ["slugify", "publish", "title", "published", "posts", "req", "state", "now", "receipt_key", "status", "route", "concat", "hmac_sha256", "JSON", "return", "if", "const"] {
            assert!(words.contains(&w.to_string()), "{w}: {words:?}");
        }
        for w in ["http_request", "collection_put", "json_stringify", "flow_start"] {
            assert!(!words.contains(&w.to_string()), "{w} is written another way");
        }
        let typed = body("  return de");
        let default = complete(&typed, at(&typed, "return de", 0, 9)).into_iter().find(|c| c.label == "default").map(|c| c.kind);
        assert_eq!(default, Some(Kind::Keyword), "a keyword, not the leaf named so, which can't be imported");
        // A js() function is the flow's; what its JavaScript binds isn't.
        let js = "import { flow, js, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nconst page = js(function html(past) {\n  const inner = past\n  return inner\n})\n\nroute.get(\"/\", () => |)\n";
        let words = offered(js);
        assert!(words.contains(&"page".to_string()) && !words.contains(&"inner".to_string()) && !words.contains(&"html".to_string()), "{words:?}");
        // After a dot: the members of what is before it.
        assert_eq!(offered(&body("  return posts.|")), ["get", "put", "delete", "list"]);
        assert_eq!(offered(&body("  return posts.l|")), ["list"]);
        assert_eq!(offered(&body("  return req.|")), ["params", "query", "headers", "body", "json"]);
        assert_eq!(offered(&body("  return state.|")), ["last_post"]);
        assert_eq!(offered(&body("  return JSON.|")), ["parse", "stringify"]);
        assert_eq!(offered(&body("  return published.|")), Vec::<String>::new(), "a value's fields aren't known");
        assert_eq!(offered(&body("  return `${req.j|}`")), ["json"], "in a template's ${{…}}");
        let state = "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"f\", { port: 8200 })\n\nroute.post(\"/a\", () => {\n  state.total = state.count\n  state.c| = 1\n})\n";
        assert_eq!(offered(state), ["count"], "the keys the text uses, not the one typed");
        let outside = "import { flow, on, route } from \"holon\"\nimport inbox from \"folder:inbox\"\nimport shop from \"database:shop\"\nimport updates from \"channel:updates\"\nimport api from \"host:api.example.com\"\n\nflow(\"f\", { port: 8200 })\n\n";
        assert_eq!(offered(&format!("{outside}route.p|")), ["post", "put", "patch"]);
        assert_eq!(offered(&format!("{outside}on.|")), ["file", "mail", "feed"]);
        assert_eq!(offered(&format!("{outside}on.file.|")), ["created", "deleted"]);
        assert_eq!(offered(&format!("{outside}route.get(\"/\", () => inbox.c|)")), ["copy", "create", "create_copy"]);
        assert_eq!(offered(&format!("{outside}route.get(\"/\", () => shop.f|)")), ["find", "find_one"]);
        assert_eq!(offered(&format!("{outside}route.get(\"/\", () => updates.|)")), ["send", "pipe", "subscribe"]);
        assert_eq!(offered(&format!("{outside}route.get(\"/\", async () => await api.p|)")), ["post", "put", "patch"]);
        // Nothing inside a string or a comment.
        assert_eq!(offered(&body("  return \"lo|")), Vec::<String>::new());
        assert_eq!(offered(&body("  return `lo|`")), Vec::<String>::new());
        assert_eq!(offered(&body("  return 1 // lo|")), Vec::<String>::new());
        assert_eq!(offered(&body("  return 1 /* lo|")), Vec::<String>::new());
        // A text cut off mid-statement still offers.
        assert!(offered("import { flow, route } from \"holon\"\nimport notes from \"collection:notes\"\n\nflow(\"f\", { port: 8200 })\n\nroute.get(\"/n\", () => no|").contains(&"notes".to_string()));
    }

    #[test]
    fn format_is_fmt() {
        assert_eq!(format(SLUGS).unwrap(), SLUGS);
        assert_eq!(format(MESSY).unwrap(), TIDY);
        assert!(format("import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.get(\"/a\", () => nope)\n").unwrap_err().contains("5:23: unknown name"));
    }
}
