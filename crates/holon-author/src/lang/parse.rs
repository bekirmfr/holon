//! The parser: tokens to the tree of ast.rs. A hand-written recursive descent with a precedence
//! climb for operators (docs/LANGUAGE.md §5). Every error names the line and column of the token
//! it stopped at, and what it expected.

use std::fmt;

use holon_core::METHODS;

use super::ast::*;
use super::lex::{lex, lex_from, LexError, Tok, Token};

#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub pos: Pos,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.pos.line, self.pos.col, self.message)
    }
}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> ParseError {
        ParseError { pos: e.pos, message: e.message }
    }
}

type R<T> = Result<T, ParseError>;

/// Words that may not begin an expression, so a missing operand is reported as such rather than
/// as an unknown name.
const NOT_A_PRIMARY: &[&str] = &["then", "else", "when"];

/// Parse a whole `.holon` text.
pub fn parse(src: &str) -> R<File> {
    Parser { toks: lex(src)?, at: 0 }.file()
}

struct Parser {
    toks: Vec<Token>,
    at: usize,
}

fn err<T>(pos: Pos, message: impl Into<String>) -> R<T> {
    Err(ParseError { pos, message: message.into() })
}

/// A token as an error message names it.
fn describe(t: &Token) -> String {
    match t.tok {
        Tok::Ident | Tok::Number | Tok::Duration(_) | Tok::Scaled(_) => format!("\"{}\"", t.text),
        Tok::Str => format!("\"\\\"{}\\\"\"", t.text),
        Tok::JsBody => "a js body".into(),
        Tok::Newline => "the end of the line".into(),
        Tok::Eof => "the end of the text".into(),
        _ => format!("\"{}\"", t.text),
    }
}

/// A JSON-style escape sequence's character, `i` at the character after the backslash.
fn unescape_at(chars: &[char], i: &mut usize) -> Option<char> {
    let c = *chars.get(*i)?;
    *i += 1;
    Some(match c {
        '"' => '"',
        '\\' => '\\',
        '/' => '/',
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'b' => '\u{8}',
        'f' => '\u{c}',
        'u' => {
            let hex: String = chars.get(*i..*i + 4)?.iter().collect();
            *i += 4;
            char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
        }
        _ => return None,
    })
}

/// With `i` just past a string's opening quote, moves it just past the closing one, over the
/// string's `{…}` parts and the strings inside them (the lexer's `string_body` rule). None when
/// the string isn't closed.
fn skip_string(chars: &[char], i: &mut usize) -> Option<()> {
    let mut depth = 0usize;
    while *i < chars.len() {
        let c = chars[*i];
        *i += 1;
        match c {
            '\\' => *i += 1,
            '"' if depth == 0 => return Some(()),
            '"' => skip_string(chars, i)?,
            '{' | '}' if depth == 0 && chars.get(*i) == Some(&c) => *i += 1,
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

impl Parser {
    // ── tokens ───────────────────────────────────────────────────────────────────────────────

    fn peek(&self) -> &Token {
        &self.toks[self.at]
    }

    fn peek_at(&self, k: usize) -> &Token {
        &self.toks[(self.at + k).min(self.toks.len() - 1)]
    }

    fn next(&mut self) -> Token {
        let t = self.toks[self.at].clone();
        if self.at < self.toks.len() - 1 {
            self.at += 1;
        }
        t
    }

    fn is(&self, tok: &Tok) -> bool {
        &self.peek().tok == tok
    }

    fn is_word(&self, w: &str) -> bool {
        self.peek().tok == Tok::Ident && self.peek().text == w
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        if self.is(tok) {
            self.next();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, tok: &Tok, what: &str) -> R<Token> {
        if self.is(tok) {
            Ok(self.next())
        } else {
            let t = self.peek();
            err(t.span.start, format!("expected {what}, found {}", describe(t)))
        }
    }

    fn expect_word(&mut self, w: &str) -> R<Token> {
        if self.is_word(w) {
            Ok(self.next())
        } else {
            let t = self.peek();
            err(t.span.start, format!("expected \"{w}\", found {}", describe(t)))
        }
    }

    fn skip_newlines(&mut self) {
        while self.is(&Tok::Newline) {
            self.next();
        }
    }

    /// An identifier, as a name.
    fn ident(&mut self, what: &str) -> R<Name> {
        if self.is(&Tok::Ident) {
            let t = self.next();
            Ok(Name { text: t.text, span: t.span })
        } else {
            let t = self.peek();
            err(t.span.start, format!("expected {what}, found {}", describe(t)))
        }
    }

    /// An identifier or a quoted string, as a name (the wiring form quotes names that aren't
    /// plain words).
    fn word_or_string(&mut self, what: &str) -> R<Name> {
        if self.is(&Tok::Str) {
            let t = self.next();
            Ok(Name { text: self.unescape(&t)?, span: t.span })
        } else {
            self.ident(what)
        }
    }

    /// A name as the wiring form writes one: a quoted string, or a plain word that may run on
    /// with dashes, dots, slashes and digits (`hello-api`, `const2`), as long as nothing separates
    /// them. Used where a name can't be mistaken for an expression: a flow's, a node's, a holon's.
    fn raw_name(&mut self, what: &str) -> R<Name> {
        if self.is(&Tok::Str) {
            return self.word_or_string(what);
        }
        if !self.is(&Tok::Ident) {
            let t = self.peek();
            return err(t.span.start, format!("expected {what}, found {}", describe(t)));
        }
        Ok(self.raw_run(|t| matches!(t, Tok::Ident | Tok::Number | Tok::Duration(_) | Tok::Minus | Tok::Dot | Tok::Slash)).expect("an ident starts it"))
    }

    /// Whether the next token past any newlines is the word `w`.
    fn word_after_newlines(&self, w: &str) -> bool {
        let mut k = 0;
        while self.peek_at(k).tok == Tok::Newline {
            k += 1;
        }
        let t = self.peek_at(k);
        t.tok == Tok::Ident && t.text == w
    }

    /// A string token's text with its escapes resolved; `{…}` parts are not read here.
    fn unescape(&self, t: &Token) -> R<String> {
        let chars: Vec<char> = t.text.chars().collect();
        let (mut out, mut i) = (String::new(), 0);
        while i < chars.len() {
            if chars[i] == '\\' {
                let at = i;
                i += 1;
                match unescape_at(&chars, &mut i) {
                    Some(c) => out.push(c),
                    None => return err(Pos { line: t.span.start.line, col: t.span.start.col + 1 + at as u32 }, "not an escape (\\\" \\\\ \\n \\t \\uXXXX)"),
                }
            } else {
                out.push(chars[i]);
                i += 1;
            }
        }
        Ok(out)
    }

    /// Adjacent tokens (no space between them) concatenated as written, while `allowed` says so
    /// and the run hasn't ended: how a route path or an origin is read without quoting.
    fn raw_run(&mut self, allowed: impl Fn(&Tok) -> bool) -> Option<Name> {
        if !allowed(&self.peek().tok) {
            return None;
        }
        let first = self.next();
        let (mut text, mut span) = (first.text.clone(), first.span);
        if first.tok == Tok::Str {
            text = format!("\"{}\"", first.text);
        }
        while allowed(&self.peek().tok) && self.peek().span.start == span.end {
            let t = self.next();
            text.push_str(&t.text);
            span.end = t.span.end;
        }
        Some(Name { text, span })
    }

    /// The end of a statement: a newline (taken), a closing brace (left for the block), or the
    /// end of the text.
    fn end_statement(&mut self) -> R<()> {
        match self.peek().tok {
            Tok::Newline => {
                self.next();
                Ok(())
            }
            Tok::RBrace | Tok::Eof => Ok(()),
            _ => {
                let t = self.peek();
                err(t.span.start, format!("unexpected {} after the statement", describe(t)))
            }
        }
    }

    // ── file ─────────────────────────────────────────────────────────────────────────────────

    fn file(&mut self) -> R<File> {
        let mut file = File::default();
        loop {
            self.skip_newlines();
            let t = self.peek().clone();
            match t.tok {
                Tok::Eof => return Ok(file),
                Tok::RBrace => return err(t.span.start, "a } without a block to close"),
                Tok::Ident => match t.text.as_str() {
                    "flow" | "port" | "every" | "interval" | "browsers" | "optional" | "collection" | "folder" | "database" | "channel" => {
                        let h = self.header()?;
                        file.headers.push(h);
                    }
                    "on" if self.peek_at(1).tok == Tok::Ident && self.peek_at(1).text == "file" => {
                        let h = self.header()?;
                        file.headers.push(h);
                    }
                    "holon" => file.items.push(self.holon()?),
                    "route" => file.items.push(Item::Route(self.route()?)),
                    _ => file.items.push(Item::Stmt(self.stmt()?)),
                },
                _ => file.items.push(Item::Stmt(self.stmt()?)),
            }
        }
    }

    fn header(&mut self) -> R<Header> {
        let head = self.next();
        let kind = match head.text.as_str() {
            "flow" => HeaderKind::Flow(self.raw_name("the flow's name")?.text),
            "port" => HeaderKind::Port(self.expect(&Tok::Number, "the port, a number")?.text),
            "interval" => HeaderKind::Interval(self.expect(&Tok::Number, "the interval in seconds, a number")?.text),
            "every" => {
                let t = self.peek().clone();
                match t.tok {
                    Tok::Duration(seconds) => {
                        self.next();
                        HeaderKind::Every { seconds, text: t.text }
                    }
                    Tok::Number => {
                        self.next();
                        let seconds = t.text.parse().map_err(|_| ParseError { pos: t.span.start, message: format!("{} is not a number", t.text) })?;
                        HeaderKind::Every { seconds, text: t.text }
                    }
                    _ => return err(t.span.start, format!("expected how often, like 10s or 5m, found {}", describe(&t))),
                }
            }
            "collection" => HeaderKind::Collection(self.ident("the collection's name")?.text),
            "folder" => HeaderKind::Folder(self.ident("the folder's name")?.text),
            "database" => HeaderKind::Database(self.ident("the database's name")?.text),
            "channel" => HeaderKind::Channel(self.ident("the channel's name")?.text),
            "on" => {
                self.next();
                let folder = self.ident("the folder whose new files start a run")?.text;
                let deleted = self.is_word("deleted");
                if deleted {
                    self.next();
                }
                let matching = if self.is_word("matching") {
                    self.next();
                    let t = self.expect(&Tok::Str, "the names to start runs for, quoted: \"*.csv\"")?;
                    Some(self.unescape(&t)?)
                } else {
                    None
                };
                HeaderKind::OnFile(FileOn { folder, deleted, matching })
            }
            "optional" => HeaderKind::Optional(self.consent()?),
            _ => {
                let mut origins = Vec::new();
                loop {
                    let Some(o) = self.raw_run(|t| !matches!(t, Tok::Comma | Tok::Newline | Tok::Eof)) else {
                        let t = self.peek();
                        return err(t.span.start, format!("expected an origin, found {}", describe(t)));
                    };
                    origins.push(if o.text.starts_with('"') { o.text.trim_matches('"').to_string() } else { o.text });
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                HeaderKind::Browsers(origins)
            }
        };
        let end = self.toks[self.at - 1].span;
        self.end_statement()?;
        Ok(Header { kind, span: head.span.to(end) })
    }

    /// What `optional` names: `secret.name`, `Name`, `Name("text")`, or the wiring form's quoted
    /// consent name.
    fn consent(&mut self) -> R<Consent> {
        if self.is(&Tok::Str) {
            let t = self.next();
            let text = self.unescape(&t)?;
            return Ok(match text.split_once('(') {
                Some((name, rest)) => Consent::Named {
                    name: name.to_string(),
                    arg: Some(rest.trim_end_matches(')').trim_matches('"').to_string()),
                },
                None => Consent::Named { name: text, arg: None },
            });
        }
        let name = self.ident("a permission: secret.name, or a consent name like HttpRequest(\"host\")")?;
        if name.text == "secret" {
            self.expect(&Tok::Dot, "\".\" then the secret's name")?;
            return Ok(Consent::Secret(self.ident("the secret's name")?.text));
        }
        let arg = if self.eat(&Tok::LParen) {
            let t = self.expect(&Tok::Str, "the permission's argument, in quotes")?;
            let arg = self.unescape(&t)?;
            self.expect(&Tok::RParen, "\")\"")?;
            Some(arg)
        } else {
            None
        };
        Ok(Consent::Named { name: name.text, arg })
    }

    // ── definitions ──────────────────────────────────────────────────────────────────────────

    /// `holon name(inputs) -> outputs { … }`, or with `js [fuel N] [memory N]` after the outputs,
    /// a holon whose body is JavaScript (docs/L4s/js-node-L4.md §2).
    fn holon(&mut self) -> R<Item> {
        let head = self.next();
        let name = self.raw_name("the holon's name")?;
        self.expect(&Tok::LParen, "\"(\" and the holon's inputs")?;
        let inputs = self.ports()?;
        self.expect(&Tok::Arrow, "\"->\" and the holon's outputs")?;
        let outputs = if self.eat(&Tok::LParen) { self.ports()? } else { vec![self.port()?] };
        if self.is(&Tok::Ident) && self.peek().text == "js" {
            self.next();
            return Ok(Item::Js(self.js(head.span, name, inputs, outputs)?));
        }
        let (body, end) = self.block()?;
        Ok(Item::Holon(HolonDef { name, inputs, outputs, body, span: head.span.to(end) }))
    }

    /// A js holon's budgets and body, after its `js`: `[fuel N] [memory N] { JavaScript }`.
    fn js(&mut self, head: Span, name: Name, inputs: Vec<PortDecl>, outputs: Vec<PortDecl>) -> R<JsDef> {
        let (mut fuel, mut memory) = (None, None);
        while self.is(&Tok::Ident) && matches!(self.peek().text.as_str(), "fuel" | "memory") {
            let which = self.next();
            let t = self.peek().clone();
            let value = match t.tok {
                Tok::Number => t.text.parse::<f64>().ok(),
                Tok::Scaled(n) => Some(n),
                _ => None,
            };
            let Some(value) = value.filter(|v| *v >= 1.0 && v.fract() == 0.0) else {
                return err(t.span.start, format!("{} takes a positive whole number (50M, 16MB, 10000000), found {}", which.text, describe(&t)));
            };
            self.next();
            let budget = Budget { value: value as u64, text: t.text.clone(), span: t.span };
            let slot = if which.text == "fuel" { &mut fuel } else { &mut memory };
            if slot.is_some() {
                return err(which.span.start, format!("{} is given twice", which.text));
            }
            *slot = Some(budget);
        }
        let body = self.expect(&Tok::JsBody, "\"{\" and the JavaScript body")?;
        Ok(JsDef { name, inputs, outputs, fuel, memory, body: body.text.clone(), body_span: body.span, span: head.to(body.span) })
    }

    /// Ports up to and including the closing parenthesis (the opening one already taken).
    fn ports(&mut self) -> R<Vec<PortDecl>> {
        let mut list = Vec::new();
        loop {
            self.skip_newlines();
            if self.eat(&Tok::RParen) {
                return Ok(list);
            }
            list.push(self.port()?);
            self.skip_newlines();
            if self.eat(&Tok::Comma) {
                continue;
            }
            if self.eat(&Tok::RParen) {
                return Ok(list);
            }
            let t = self.peek();
            return err(t.span.start, format!("expected \",\" or \")\" after the port, found {}", describe(t)));
        }
    }

    fn port(&mut self) -> R<PortDecl> {
        let name = self.word_or_string("a port's name")?;
        let ty = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
        let unit = if self.is(&Tok::Str) {
            let t = self.next();
            Some(self.unescape(&t)?)
        } else {
            None
        };
        Ok(PortDecl { name, ty, unit })
    }

    fn type_expr(&mut self) -> R<TypeExpr> {
        let name = self.ident("a type (Text, Number, Boolean, Record, List<…>, Any)")?;
        let mut args = Vec::new();
        let mut span = name.span;
        if self.eat(&Tok::Lt) {
            loop {
                args.push(self.type_expr()?);
                if self.eat(&Tok::Comma) {
                    continue;
                }
                span.end = self.expect(&Tok::Gt, "\">\" closing the type")?.span.end;
                break;
            }
        }
        Ok(TypeExpr { name: name.text, args, span })
    }

    fn route(&mut self) -> R<RouteDef> {
        let head = self.next();
        let method = self.ident("a method (GET, POST, PUT, PATCH, DELETE)")?;
        if !METHODS.contains(&method.text.as_str()) {
            return err(method.span.start, format!("\"{}\" is not a method ({})", method.text, METHODS.join(", ")));
        }
        if !self.is(&Tok::Slash) {
            let t = self.peek();
            return err(t.span.start, format!("expected a route path like /todos/:id, found {}", describe(t)));
        }
        let path = self
            .raw_run(|t| matches!(t, Tok::Slash | Tok::Ident | Tok::Number | Tok::Duration(_) | Tok::Colon | Tok::Dot | Tok::Minus))
            .expect("a slash starts it");
        self.expect(&Tok::LParen, "\"(\" and a name for the request")?;
        let request = self.ident("a name for the request")?;
        self.expect(&Tok::RParen, "\")\"")?;
        let resumes = if self.is_word("resumes") {
            self.next();
            Some(self.ident("the waiting point this route resumes")?)
        } else {
            None
        };
        let (body, end) = self.block()?;
        Ok(RouteDef { method, path, request, resumes, body, span: head.span.to(end) })
    }

    /// `{ statements }`, and the span of the closing brace.
    fn block(&mut self) -> R<(Vec<Stmt>, Span)> {
        let open = self.expect(&Tok::LBrace, "\"{\"")?;
        let mut body = Vec::new();
        loop {
            self.skip_newlines();
            if self.is(&Tok::RBrace) {
                let close = self.next();
                return Ok((body, close.span));
            }
            if self.is(&Tok::Eof) {
                return err(open.span.start, "the block isn't closed with }");
            }
            body.push(self.stmt()?);
        }
    }

    // ── statements ───────────────────────────────────────────────────────────────────────────

    fn stmt(&mut self) -> R<Stmt> {
        let start = self.peek().span;
        let kind = if self.is_word("node") {
            self.node_stmt()?
        } else if self.is_word("delete") {
            self.next();
            let collection = self.ident("the collection to delete from")?;
            self.expect(&Tok::LBracket, "\"[\" and the key to delete")?;
            let key = self.expr()?;
            self.expect(&Tok::RBracket, "\"]\"")?;
            StmtKind::Delete { collection, key }
        } else if self.is_word("respond") && self.peek_at(1).tok != Tok::Dot {
            self.next();
            let args = if matches!(self.peek().tok, Tok::Newline | Tok::RBrace | Tok::Eof) { vec![] } else { self.loose_args()? };
            StmtKind::Respond(args)
        } else {
            let e = self.expr()?;
            if self.is(&Tok::Eq) {
                self.next();
                self.skip_newlines();
                let value = self.binding_value()?;
                match e.kind {
                    ExprKind::Name(text) => StmtKind::Bind { name: Name { text, span: e.span }, value },
                    ExprKind::State(key) => StmtKind::StateWrite { key: Name { text: key, span: e.span }, value },
                    ExprKind::Noun(n) => {
                        let word = match n {
                            Noun::Now => "now",
                            Noun::Random => "random",
                            Noun::Ingress => "ingress",
                        };
                        return err(e.span.start, format!("\"{word}\" is a keyword; a binding may not shadow it"));
                    }
                    ExprKind::Index { base, key } => match base.kind {
                        ExprKind::Name(text) => StmtKind::CollectionWrite { collection: Name { text, span: base.span }, key: *key, value },
                        _ => return err(e.span.start, "can't assign to this: a name, state.key, or collection[key]"),
                    },
                    _ => return err(e.span.start, "can't assign to this: a name, state.key, or collection[key]"),
                }
            } else if self.is(&Tok::Arrow) {
                self.next();
                let from = Self::wire_end(e)?;
                let to = Self::wire_end(self.expr()?)?;
                StmtKind::Wire { from, to }
            } else {
                StmtKind::Expr(e)
            }
        };
        let end = self.toks[self.at - 1].span;
        self.end_statement()?;
        Ok(Stmt { kind, span: start.to(end) })
    }

    /// A binding's right side: an expression, or `await name, expires: expr`.
    fn binding_value(&mut self) -> R<Expr> {
        let mut e = self.expr()?;
        if let ExprKind::Await { expires, .. } = &mut e.kind
            && self.is(&Tok::Comma)
        {
            self.next();
            self.skip_newlines();
            self.expect_word("expires")?;
            self.expect(&Tok::Colon, "\":\" and how long the wait may last")?;
            let how_long = self.expr()?;
            e.span.end = how_long.span.end;
            *expires = Some(Box::new(how_long));
        }
        Ok(e)
    }

    fn wire_end(e: Expr) -> R<(Name, Name)> {
        match e.kind {
            ExprKind::Field { base, name } => match base.kind {
                ExprKind::Name(text) => Ok((Name { text, span: base.span }, name)),
                _ => err(e.span.start, "a wire's end is node.port"),
            },
            ExprKind::State(key) => Ok((Name { text: "state".into(), span: e.span }, Name { text: key, span: e.span })),
            ExprKind::Secret(name) => Ok((Name { text: "secret".into(), span: e.span }, Name { text: name, span: e.span })),
            _ => err(e.span.start, "a wire's end is node.port"),
        }
    }

    /// `node name = of[arity] { k: v, … } at x y`.
    fn node_stmt(&mut self) -> R<StmtKind> {
        self.next();
        let name = self.raw_name("the node's name")?;
        self.expect(&Tok::Eq, "\"=\" and what the node is")?;
        let of = self.raw_name("what the node is (a leaf, a template or a holon)")?;
        let mut arity = None;
        if self.eat(&Tok::LBracket) {
            let n = self.expect(&Tok::Number, "how many inputs")?;
            arity = Some(n.text.parse().map_err(|_| ParseError { pos: n.span.start, message: "inputs: a whole number".into() })?);
            self.expect(&Tok::RBracket, "\"]\"")?;
        }
        let mut params = Vec::new();
        if self.eat(&Tok::LBrace) {
            loop {
                self.skip_newlines();
                if self.eat(&Tok::RBrace) {
                    break;
                }
                let key = self.word_or_string("a param's name")?;
                self.expect(&Tok::Colon, "\":\" and the param's value")?;
                let value = self.param_value(&key)?;
                params.push((key, value));
                self.skip_newlines();
                if self.eat(&Tok::Comma) {
                    continue;
                }
                if self.eat(&Tok::RBrace) {
                    break;
                }
                let t = self.peek();
                return err(t.span.start, format!("expected \",\" or \"}}\" after the param, found {}", describe(t)));
            }
        }
        let mut at = None;
        if self.is_word("at") {
            self.next();
            let x = self.signed_number("x")?;
            let y = self.signed_number("y")?;
            at = Some((x, y));
        }
        Ok(StmtKind::Node { name, of, arity, params, at })
    }

    /// A wiring-form param value: a string (unescaped), or whatever is written up to the comma.
    fn param_value(&mut self, key: &Name) -> R<String> {
        if self.is(&Tok::Str) {
            let t = self.next();
            return self.unescape(&t);
        }
        match self.raw_run(|t| !matches!(t, Tok::Comma | Tok::RBrace | Tok::Newline | Tok::Eof)) {
            Some(v) => Ok(v.text),
            None => {
                let t = self.peek();
                err(t.span.start, format!("expected {}'s value, found {}", key.text, describe(t)))
            }
        }
    }

    fn signed_number(&mut self, what: &str) -> R<f64> {
        let neg = self.eat(&Tok::Minus);
        let t = self.expect(&Tok::Number, &format!("{what}, a number"))?;
        let n: f64 = t.text.parse().map_err(|_| ParseError { pos: t.span.start, message: format!("{} is not a number", t.text) })?;
        Ok(if neg { -n } else { n })
    }

    // ── expressions ──────────────────────────────────────────────────────────────────────────

    fn expr(&mut self) -> R<Expr> {
        self.binary(0)
    }

    fn binop_here(&self) -> Option<BinOp> {
        let t = self.peek();
        Some(match t.tok {
            Tok::Ident if t.text == "when" => BinOp::When,
            Tok::QQ => BinOp::Coalesce,
            Tok::OrOr => BinOp::Or,
            Tok::AndAnd => BinOp::And,
            Tok::EqEq => BinOp::Eq,
            Tok::NotEq => BinOp::Ne,
            Tok::Lt => BinOp::Lt,
            Tok::Le => BinOp::Le,
            Tok::Gt => BinOp::Gt,
            Tok::Ge => BinOp::Ge,
            Tok::Plus => BinOp::Add,
            Tok::Minus => BinOp::Sub,
            Tok::Star => BinOp::Mul,
            Tok::Slash => BinOp::Div,
            _ => return None,
        })
    }

    /// Precedence climbing: operators of higher precedence bind tighter, equal ones to the left.
    fn binary(&mut self, min: u8) -> R<Expr> {
        let mut lhs = self.unary()?;
        while let Some(op) = self.binop_here() {
            if op.precedence() <= min {
                break;
            }
            self.next();
            self.skip_newlines();
            let rhs = self.binary(op.precedence())?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr { kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> R<Expr> {
        if self.is(&Tok::Bang) {
            let bang = self.next();
            let e = self.unary()?;
            let span = bang.span.to(e.span);
            return Ok(Expr { kind: ExprKind::Not(Box::new(e)), span });
        }
        if self.is(&Tok::Minus) {
            let minus = self.next();
            let t = self.peek().clone();
            return match t.tok {
                Tok::Number | Tok::Duration(_) => {
                    let mut e = self.primary()?;
                    match &mut e.kind {
                        ExprKind::Number { value, text } | ExprKind::Duration { seconds: value, text } => {
                            *value = -*value;
                            text.insert(0, '-');
                        }
                        _ => unreachable!("a number or duration token"),
                    }
                    e.span = minus.span.to(e.span);
                    Ok(e)
                }
                _ => err(minus.span.start, "a minus needs a number after it (to subtract, write a - b)"),
            };
        }
        let e = self.primary()?;
        self.postfix(e)
    }

    fn postfix(&mut self, mut e: Expr) -> R<Expr> {
        loop {
            if self.is(&Tok::Dot) {
                self.next();
                let name = self.word_or_string("a field or output name")?;
                let span = e.span.to(name.span);
                e = Expr { kind: ExprKind::Field { base: Box::new(e), name }, span };
            } else if self.is(&Tok::LBracket) {
                self.next();
                self.skip_newlines();
                let key = self.expr()?;
                self.skip_newlines();
                let close = self.expect(&Tok::RBracket, "\"]\"")?;
                let span = e.span.to(close.span);
                e = Expr { kind: ExprKind::Index { base: Box::new(e), key: Box::new(key) }, span };
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> R<Expr> {
        let t = self.peek().clone();
        let lit = |kind: ExprKind, span: Span| Ok(Expr { kind, span });
        match t.tok {
            Tok::Number => {
                self.next();
                let value = t.text.parse().map_err(|_| ParseError { pos: t.span.start, message: format!("{} is not a number", t.text) })?;
                lit(ExprKind::Number { value, text: t.text }, t.span)
            }
            Tok::Duration(seconds) => {
                self.next();
                lit(ExprKind::Duration { seconds, text: t.text }, t.span)
            }
            Tok::Str => {
                self.next();
                self.string(&t)
            }
            Tok::LParen => {
                self.next();
                self.skip_newlines();
                let inner = self.expr()?;
                self.skip_newlines();
                let close = self.expect(&Tok::RParen, "\")\"")?;
                lit(ExprKind::Paren(Box::new(inner)), t.span.to(close.span))
            }
            Tok::LBrace => self.record(),
            Tok::LBracket => self.list(),
            Tok::Ident => self.word(t),
            _ => err(t.span.start, format!("expected an expression, found {}", describe(&t))),
        }
    }

    /// An expression that starts with a word: a literal, a noun, a keyword form, a call, a name.
    fn word(&mut self, t: Token) -> R<Expr> {
        let span = t.span;
        let lit = |kind: ExprKind| Ok(Expr { kind, span });
        // The wiring form names nodes after their leaves, so `await.out -> via.record` names a
        // node called `await`: a keyword followed by a dot is a plain name.
        if self.peek_at(1).tok == Tok::Dot && matches!(t.text.as_str(), "await" | "now" | "random" | "ingress" | "if" | "match" | "fn" | "true" | "false") {
            self.next();
            return lit(ExprKind::Name(t.text));
        }
        match t.text.as_str() {
            "true" | "false" => {
                self.next();
                lit(ExprKind::Bool(t.text == "true"))
            }
            "now" => {
                self.next();
                lit(ExprKind::Noun(Noun::Now))
            }
            "random" => {
                self.next();
                lit(ExprKind::Noun(Noun::Random))
            }
            "ingress" => {
                self.next();
                lit(ExprKind::Noun(Noun::Ingress))
            }
            "if" => self.if_expr(),
            "match" => self.match_expr(),
            "fn" => self.lambda(),
            "await" => {
                self.next();
                let name = self.ident("the waiting point's name")?;
                Ok(Expr { kind: ExprKind::Await { name: name.clone(), expires: None }, span: span.to(name.span) })
            }
            w if NOT_A_PRIMARY.contains(&w) => err(span.start, format!("expected an expression, found \"{w}\"")),
            "secret" | "state" | "http" | "stripe" if self.peek_at(1).tok == Tok::Dot => {
                self.next();
                self.next();
                let member = self.ident(&format!("a name after {}.", t.text))?;
                let receiver = Name { text: t.text.clone(), span };
                if self.is(&Tok::LParen) {
                    let (args, end) = self.args()?;
                    return Ok(Expr { kind: ExprKind::Method { receiver, method: member, args }, span: span.to(end) });
                }
                let whole = span.to(member.span);
                match t.text.as_str() {
                    "secret" => Ok(Expr { kind: ExprKind::Secret(member.text), span: whole }),
                    "state" => Ok(Expr { kind: ExprKind::State(member.text), span: whole }),
                    ns => err(span.start, format!("{ns}.{} is a call: {ns}.{}(…)", member.text, member.text)),
                }
            }
            "secret" | "state" => err(span.start, format!("{} takes a name: {}.{}", t.text, t.text, if t.text == "secret" { "api_token" } else { "key" })),
            _ => {
                self.next();
                let name = Name { text: t.text.clone(), span };
                if self.is(&Tok::Dot) && self.peek_at(1).tok == Tok::Ident && self.peek_at(2).tok == Tok::LParen {
                    self.next();
                    let method = self.ident("a method")?;
                    let (args, end) = self.args()?;
                    return Ok(Expr { kind: ExprKind::Method { receiver: name, method, args }, span: span.to(end) });
                }
                if self.is(&Tok::LParen) {
                    let (args, end) = self.args()?;
                    return Ok(Expr { kind: ExprKind::Call { callee: name, args }, span: span.to(end) });
                }
                lit(ExprKind::Name(t.text))
            }
        }
    }

    fn if_expr(&mut self) -> R<Expr> {
        let head = self.next();
        let cond = self.expr()?;
        if !self.is_word("then") {
            let t = self.peek();
            return err(t.span.start, format!("expected \"then\" after the condition, found {}", describe(t)));
        }
        self.next();
        self.skip_newlines();
        let then = self.expr()?;
        let mut span = head.span.to(then.span);
        // `else` may start the next line.
        let els = if self.word_after_newlines("else") {
            self.skip_newlines();
            self.next();
            self.skip_newlines();
            let e = self.expr()?;
            span = span.to(e.span);
            Some(Box::new(e))
        } else {
            None
        };
        Ok(Expr { kind: ExprKind::If { cond: Box::new(cond), then: Box::new(then), els }, span })
    }

    /// `match subject { arms }`: arms separated by newlines or commas, each `patterns -> value`,
    /// and `else -> value` last. Whether a pattern is a literal is the lowering's to say.
    fn match_expr(&mut self) -> R<Expr> {
        let head = self.next();
        let subject = self.expr()?;
        self.skip_newlines();
        if !self.is(&Tok::LBrace) {
            let t = self.peek();
            return err(t.span.start, format!("expected \"{{\" and the arms after the subject, found {}", describe(t)));
        }
        let open = self.next().span;
        let mut arms = Vec::new();
        let mut els: Option<Box<Expr>> = None;
        let close = loop {
            self.skip_newlines();
            if self.is(&Tok::RBrace) {
                break self.next().span;
            }
            if els.is_some() {
                return err(self.peek().span.start, "`else` comes last, once");
            }
            if self.is_word("else") {
                self.next();
                self.expect(&Tok::Arrow, "\"->\" and the value when no pattern matches")?;
                self.skip_newlines();
                els = Some(Box::new(self.expr()?));
            } else {
                let mut patterns = vec![self.expr()?];
                while self.eat(&Tok::Comma) {
                    self.skip_newlines();
                    patterns.push(self.expr()?);
                }
                self.expect(&Tok::Arrow, "\"->\" and the arm's value")?;
                self.skip_newlines();
                let value = self.expr()?;
                arms.push(Arm { patterns, value });
            }
            if self.eat(&Tok::Comma) || self.is(&Tok::Newline) || self.is(&Tok::RBrace) {
                continue;
            }
            let t = self.peek();
            return err(t.span.start, format!("expected a new line, \",\" or \"}}\" after the arm, found {}", describe(t)));
        };
        if arms.is_empty() {
            return err(open.start, "a `match` needs an arm");
        }
        Ok(Expr { kind: ExprKind::Match { subject: Box::new(subject), arms, els, open }, span: head.span.to(close) })
    }

    fn lambda(&mut self) -> R<Expr> {
        let head = self.next();
        self.expect(&Tok::LParen, "\"(\" and the function's parameters")?;
        let mut params = Vec::new();
        loop {
            self.skip_newlines();
            if self.eat(&Tok::RParen) {
                break;
            }
            params.push(self.ident("a parameter's name")?);
            self.skip_newlines();
            if self.eat(&Tok::Comma) {
                continue;
            }
            self.expect(&Tok::RParen, "\",\" or \")\"")?;
            break;
        }
        self.expect(&Tok::Arrow, "\"->\" and the function's body")?;
        self.skip_newlines();
        let body = self.expr()?;
        let span = head.span.to(body.span);
        Ok(Expr { kind: ExprKind::Lambda { params, body: Box::new(body) }, span })
    }

    /// `(args)`: positional expressions and `name: value` pairs. The span of the closing paren.
    fn args(&mut self) -> R<(Vec<Arg>, Span)> {
        self.expect(&Tok::LParen, "\"(\"")?;
        let mut args = Vec::new();
        loop {
            self.skip_newlines();
            if self.is(&Tok::RParen) {
                return Ok((args, self.next().span));
            }
            args.push(self.arg()?);
            self.skip_newlines();
            if self.eat(&Tok::Comma) {
                continue;
            }
            if self.is(&Tok::RParen) {
                return Ok((args, self.next().span));
            }
            let t = self.peek();
            return err(t.span.start, format!("expected \",\" or \")\" after the argument, found {}", describe(t)));
        }
    }

    /// `respond`'s arguments: as a call's, without parentheses, to the end of the line.
    fn loose_args(&mut self) -> R<Vec<Arg>> {
        let mut args = vec![self.arg()?];
        while self.eat(&Tok::Comma) {
            self.skip_newlines();
            args.push(self.arg()?);
        }
        Ok(args)
    }

    fn arg(&mut self) -> R<Arg> {
        if self.is(&Tok::Ident) && self.peek_at(1).tok == Tok::Colon {
            let name = self.ident("an argument's name")?;
            self.next();
            self.skip_newlines();
            let value = self.expr()?;
            return Ok(Arg { name: Some(name), value });
        }
        Ok(Arg { name: None, value: self.expr()? })
    }

    fn record(&mut self) -> R<Expr> {
        let open = self.next();
        self.skip_newlines();
        let mut spread = None;
        if self.eat(&Tok::DotDot) {
            spread = Some(Box::new(self.expr()?));
            self.skip_newlines();
            if !self.is(&Tok::RBrace) {
                self.expect(&Tok::Comma, "\",\" after the record spread, or \"}\"")?;
            }
        }
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if self.is(&Tok::RBrace) {
                let close = self.next();
                return Ok(Expr { kind: ExprKind::Record { spread, fields }, span: open.span.to(close.span) });
            }
            let key = match self.peek().tok {
                Tok::Ident => {
                    let t = self.next();
                    Key { text: t.text, quoted: false, span: t.span }
                }
                Tok::Str => {
                    let t = self.next();
                    Key { text: self.unescape(&t)?, quoted: true, span: t.span }
                }
                _ => {
                    let t = self.peek();
                    return err(t.span.start, format!("expected a field name, found {}", describe(t)));
                }
            };
            self.expect(&Tok::Colon, "\":\" and the field's value")?;
            self.skip_newlines();
            let value = self.expr()?;
            fields.push((key, value));
            self.skip_newlines();
            if self.eat(&Tok::Comma) {
                continue;
            }
            if !self.is(&Tok::RBrace) {
                let t = self.peek();
                return err(t.span.start, format!("expected \",\" or \"}}\" after the field, found {}", describe(t)));
            }
        }
    }

    fn list(&mut self) -> R<Expr> {
        let open = self.next();
        let mut items = Vec::new();
        loop {
            self.skip_newlines();
            if self.is(&Tok::RBracket) {
                let close = self.next();
                return Ok(Expr { kind: ExprKind::List(items), span: open.span.to(close.span) });
            }
            items.push(self.expr()?);
            self.skip_newlines();
            if self.eat(&Tok::Comma) {
                continue;
            }
            if !self.is(&Tok::RBracket) {
                let t = self.peek();
                return err(t.span.start, format!("expected \",\" or \"]\" after the item, found {}", describe(t)));
            }
        }
    }

    /// A string token as an expression: its text, or its pieces when it holds `{…}` parts, each
    /// part parsed as an expression with the columns of where it sits in the string.
    fn string(&mut self, t: &Token) -> R<Expr> {
        let chars: Vec<char> = t.text.chars().collect();
        // The column of the string's character `i`: the quote, then the characters before it.
        let col_of = |i: usize| Pos { line: t.span.start.line, col: t.span.start.col + 1 + i as u32 };
        let (mut parts, mut text, mut i) = (Vec::<Part>::new(), String::new(), 0);
        while i < chars.len() {
            match chars[i] {
                '\\' => {
                    let at = i;
                    i += 1;
                    match unescape_at(&chars, &mut i) {
                        Some(c) => text.push(c),
                        None => return err(col_of(at), "not an escape (\\\" \\\\ \\n \\t \\uXXXX)"),
                    }
                }
                '{' if chars.get(i + 1) == Some(&'{') => {
                    text.push('{');
                    i += 2;
                }
                '}' if chars.get(i + 1) == Some(&'}') => {
                    text.push('}');
                    i += 2;
                }
                '}' => return err(col_of(i), "a } in a string needs a { before it (write }} for a brace)"),
                '{' => {
                    let open = i;
                    i += 1;
                    let (mut depth, start) = (1, i);
                    while i < chars.len() && depth > 0 {
                        match chars[i] {
                            '{' => depth += 1,
                            '}' => depth -= 1,
                            // A string inside the part: its braces are its own.
                            '"' => {
                                i += 1;
                                if skip_string(&chars, &mut i).is_none() {
                                    return err(col_of(open), "a { in a string isn't closed with } (write {{ for a brace)");
                                }
                                continue;
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                    if depth > 0 {
                        return err(col_of(open), "a { in a string isn't closed with } (write {{ for a brace)");
                    }
                    let inner: String = chars[start..i - 1].iter().collect();
                    if inner.trim().is_empty() {
                        return err(col_of(open), "an empty {} in a string: put an expression inside, or write {{}} for braces");
                    }
                    if !text.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut text)));
                    }
                    let mut sub = Parser { toks: lex_from(&inner, col_of(start))?, at: 0 };
                    sub.skip_newlines();
                    let e = sub.expr()?;
                    sub.skip_newlines();
                    if !sub.is(&Tok::Eof) {
                        let u = sub.peek();
                        return err(u.span.start, format!("unexpected {} inside {{…}}", describe(u)));
                    }
                    parts.push(Part::Expr(e));
                }
                c => {
                    text.push(c);
                    i += 1;
                }
            }
        }
        if parts.is_empty() {
            return Ok(Expr { kind: ExprKind::Text(text), span: t.span });
        }
        if !text.is_empty() {
            parts.push(Part::Text(text));
        }
        Ok(Expr { kind: ExprKind::Interpolated(parts), span: t.span })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{commands, samples, text};

    fn file(src: &str) -> File {
        parse(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    fn one_stmt(src: &str) -> StmtKind {
        let f = file(src);
        assert_eq!(f.items.len(), 1, "{src}");
        match f.items.into_iter().next().unwrap() {
            Item::Stmt(s) => s.kind,
            other => panic!("not a statement: {other:?}"),
        }
    }

    fn one_expr(src: &str) -> ExprKind {
        match one_stmt(src) {
            StmtKind::Expr(e) => e.kind,
            other => panic!("not an expression: {other:?}"),
        }
    }

    /// An expression printed back with every binary operation and parenthesis explicit, so a
    /// test says exactly how the operators grouped.
    fn show(e: &ExprKind) -> String {
        match e {
            ExprKind::Number { text, .. } | ExprKind::Duration { text, .. } => text.clone(),
            ExprKind::Text(s) => format!("{s:?}"),
            ExprKind::Interpolated(parts) => parts
                .iter()
                .map(|p| match p {
                    Part::Text(s) => format!("{s:?}"),
                    Part::Expr(e) => format!("{{{}}}", show(&e.kind)),
                })
                .collect::<Vec<_>>()
                .join("+"),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Name(n) => n.clone(),
            ExprKind::Secret(n) => format!("secret.{n}"),
            ExprKind::State(n) => format!("state.{n}"),
            ExprKind::Noun(n) => format!("{n:?}").to_lowercase(),
            ExprKind::Call { callee, args } => format!("{}({})", callee.text, show_args(args)),
            ExprKind::Method { receiver, method, args } => format!("{}.{}({})", receiver.text, method.text, show_args(args)),
            ExprKind::Field { base, name } => format!("{}.{}", show(&base.kind), name.text),
            ExprKind::Index { base, key } => format!("{}[{}]", show(&base.kind), show(&key.kind)),
            ExprKind::Record { spread, fields } => {
                let mut items: Vec<String> = spread.iter().map(|s| format!("..{}", show(&s.kind))).collect();
                items.extend(fields.iter().map(|(k, v)| format!("{}{}: {}", k.text, if k.quoted { "!" } else { "" }, show(&v.kind))));
                format!("{{{}}}", items.join(", "))
            }
            ExprKind::List(items) => format!("[{}]", items.iter().map(|i| show(&i.kind)).collect::<Vec<_>>().join(", ")),
            ExprKind::Not(e) => format!("!{}", show(&e.kind)),
            ExprKind::Binary { op, lhs, rhs } => format!("<{} {} {}>", show(&lhs.kind), op.text(), show(&rhs.kind)),
            ExprKind::If { cond, then, els } => match els {
                Some(e) => format!("if {} then {} else {}", show(&cond.kind), show(&then.kind), show(&e.kind)),
                None => format!("if {} then {}", show(&cond.kind), show(&then.kind)),
            },
            ExprKind::Match { subject, arms, els, .. } => {
                let mut parts: Vec<String> = arms
                    .iter()
                    .map(|a| format!("{} -> {}", a.patterns.iter().map(|p| show(&p.kind)).collect::<Vec<_>>().join(", "), show(&a.value.kind)))
                    .collect();
                parts.extend(els.iter().map(|e| format!("else -> {}", show(&e.kind))));
                format!("match {} {{ {} }}", show(&subject.kind), parts.join("; "))
            }
            ExprKind::Await { name, expires } => match expires {
                Some(e) => format!("await {} expires {}", name.text, show(&e.kind)),
                None => format!("await {}", name.text),
            },
            ExprKind::Lambda { params, body } => format!("fn({}) -> {}", params.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(", "), show(&body.kind)),
            ExprKind::Paren(e) => format!("({})", show(&e.kind)),
        }
    }

    fn show_args(args: &[Arg]) -> String {
        args.iter()
            .map(|a| match &a.name {
                Some(n) => format!("{}: {}", n.text, show(&a.value.kind)),
                None => show(&a.value.kind),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn e(src: &str) -> String {
        show(&one_expr(src))
    }

    fn error(src: &str) -> (u32, u32, String) {
        let e = parse(src).expect_err(src);
        (e.pos.line, e.pos.col, e.message)
    }

    // ── headers ──

    #[test]
    fn headers_in_both_forms() {
        let f = file("flow hello-api\nport 8101\nevery 10s\ninterval 10\nbrowsers https://a.example, http://localhost:5173\noptional secret.receipt_key\noptional HttpRequest(\"api.example.com\")\noptional \"SecretRead(\\\"receipt_key\\\")\"\ncollection notes\n");
        let kinds: Vec<&HeaderKind> = f.headers.iter().map(|h| &h.kind).collect();
        assert_eq!(kinds.len(), 9);
        assert_eq!(kinds[0], &HeaderKind::Flow("hello-api".into()));
        assert_eq!(kinds[1], &HeaderKind::Port("8101".into()));
        assert_eq!(kinds[2], &HeaderKind::Every { seconds: 10.0, text: "10s".into() });
        assert_eq!(kinds[3], &HeaderKind::Interval("10".into()));
        assert_eq!(kinds[4], &HeaderKind::Browsers(vec!["https://a.example".into(), "http://localhost:5173".into()]));
        assert_eq!(kinds[5], &HeaderKind::Optional(Consent::Secret("receipt_key".into())));
        assert_eq!(kinds[6], &HeaderKind::Optional(Consent::Named { name: "HttpRequest".into(), arg: Some("api.example.com".into()) }));
        assert_eq!(kinds[7], &HeaderKind::Optional(Consent::Named { name: "SecretRead".into(), arg: Some("receipt_key".into()) }));
        assert_eq!(kinds[8], &HeaderKind::Collection("notes".into()));
        assert!(f.items.is_empty());
        // `every` also takes a plain number of seconds; `flow` a quoted name.
        assert_eq!(file("every 300\nflow \"my flow\"").headers[0].kind, HeaderKind::Every { seconds: 300.0, text: "300".into() });
    }

    // ── definitions ──

    #[test]
    fn a_holon_in_the_expression_form_with_typed_ports() {
        let f = file("holon slugify(text: Text) -> (slug: Text) {\n  slug = join(split(lowercase(text), \" \"), \"-\")\n}\n");
        let Item::Holon(h) = &f.items[0] else { panic!() };
        assert_eq!(h.name.text, "slugify");
        assert_eq!(h.inputs.len(), 1);
        assert_eq!(h.inputs[0].name.text, "text");
        assert_eq!(h.inputs[0].ty.as_ref().unwrap().name, "Text");
        assert_eq!(h.outputs[0].name.text, "slug");
        assert_eq!(h.body.len(), 1);
        assert!(matches!(&h.body[0].kind, StmtKind::Bind { name, .. } if name.text == "slug"));
        assert_eq!((h.span.start.line, h.span.end.line), (1, 3));
        // Several outputs, a List type, a single untyped output without parentheses.
        let f = file("holon publish(slug, author) -> (put, saved: Effect, receipt: List<Text>) {\n}\nholon total(prices) -> sum {\n sum = fold(0, prices, fn: fn(acc, p) -> acc + p)\n}");
        let Item::Holon(h) = &f.items[0] else { panic!() };
        assert_eq!(h.outputs.len(), 3);
        assert_eq!(h.outputs[2].ty.as_ref().unwrap().args[0].name, "Text");
        let Item::Holon(t) = &f.items[1] else { panic!() };
        assert_eq!(t.outputs.len(), 1);
    }

    #[test]
    fn a_holon_in_the_wiring_form_with_unit_names() {
        let f = file("holon slugify(text \"s\") -> (slug \"out\") {\n  node lower = lowercase at 250 0\n  node const = const { type: Text, value: \" \" } at 250 150\n  in.text -> lower.s\n  lower.out -> out.slug\n}\n");
        let Item::Holon(h) = &f.items[0] else { panic!() };
        assert_eq!(h.inputs[0].unit.as_deref(), Some("s"));
        assert_eq!(h.outputs[0].unit.as_deref(), Some("out"));
        assert_eq!(h.body.len(), 4);
        assert!(matches!(&h.body[0].kind, StmtKind::Node { name, of, at: Some((250.0, 0.0)), .. } if name.text == "lower" && of.text == "lowercase"));
        assert!(matches!(&h.body[1].kind, StmtKind::Node { params, .. } if params.len() == 2 && params[1].1 == " "));
        assert!(matches!(&h.body[2].kind, StmtKind::Wire { from, to } if from.0.text == "in" && from.1.text == "text" && to.0.text == "lower" && to.1.text == "s"));
    }

    #[test]
    fn a_route_with_its_request_name_and_a_resume() {
        let f = file("route GET /paid/:token (req) resumes paid {\n  respond \"thanks\"\n}\nroute POST /orders (req) {\n}\n");
        let Item::Route(r) = &f.items[0] else { panic!() };
        assert_eq!((r.method.text.as_str(), r.path.text.as_str(), r.request.text.as_str()), ("GET", "/paid/:token", "req"));
        assert_eq!(r.resumes.as_ref().unwrap().text, "paid");
        assert_eq!(r.body.len(), 1);
        let Item::Route(r) = &f.items[1] else { panic!() };
        assert_eq!(r.path.text, "/orders");
        assert!(r.resumes.is_none() && r.body.is_empty());
        // A path with a version, a dash and a dot.
        let Item::Route(r) = &file("route GET /v2/a-b/x.json (r) {}").items[0] else { panic!() };
        assert_eq!(r.path.text, "/v2/a-b/x.json");
    }

    // ── statements ──

    #[test]
    fn every_statement_form() {
        assert!(matches!(one_stmt("n = 1"), StmtKind::Bind { name, .. } if name.text == "n"));
        assert!(matches!(one_stmt("state.beats = (state.beats ?? 0) + 1"), StmtKind::StateWrite { key, .. } if key.text == "beats"));
        assert!(matches!(one_stmt("notes[req.params.id] = req.json"), StmtKind::CollectionWrite { collection, .. } if collection.text == "notes"));
        assert!(matches!(one_stmt("delete notes[id]"), StmtKind::Delete { collection, .. } if collection.text == "notes"));
        assert!(matches!(one_stmt("egress(x)"), StmtKind::Expr(_)));
        assert!(matches!(one_stmt("node greet = concat[3]"), StmtKind::Node { arity: Some(3), .. }));
        assert!(matches!(one_stmt("hi.out -> greet.a"), StmtKind::Wire { .. }));
        // Nodes named after leaves that are keywords here: still wires.
        assert!(matches!(one_stmt("await.out -> via.record"), StmtKind::Wire { from, .. } if from.0.text == "await"));
        assert!(matches!(one_stmt("random.out -> respond.body"), StmtKind::Wire { to, .. } if to.0.text == "respond"));
        assert!(matches!(one_stmt("respond.response -> x.a"), StmtKind::Wire { from, .. } if from.0.text == "respond"));
        assert!(matches!(one_stmt("secret.out -> verify.secret"), StmtKind::Wire { from, .. } if from.0.text == "secret" && from.1.text == "out"));
        // Statement spans cover the whole line.
        let f = file("  x = 1 + 2\n");
        let Item::Stmt(s) = &f.items[0] else { panic!() };
        assert_eq!((s.span.start.col, s.span.end.col), (3, 12));
    }

    #[test]
    fn respond_in_every_shape() {
        let args = |src: &str| match one_stmt(src) {
            StmtKind::Respond(a) => show_args(&a),
            other => panic!("{other:?}"),
        };
        assert_eq!(args("respond"), "");
        assert_eq!(args("respond 204"), "204");
        assert_eq!(args("respond status: 204"), "status: 204");
        assert_eq!(args("respond body: 42"), "body: 42");
        assert_eq!(args("respond req.json, missing: 400"), "req.json, missing: 400");
        assert_eq!(args("respond { receipt: receipt } when ok, missing: 401"), "<{receipt: receipt} when ok>, missing: 401");
        assert_eq!(args("respond x,\n  status: 200,\n  missing: 404"), "x, status: 200, missing: 404");
    }

    #[test]
    fn a_binding_to_await_takes_how_long_it_may_last() {
        assert!(matches!(one_stmt("paid = await paid, expires: 1d"), StmtKind::Bind { value: Expr { kind: ExprKind::Await { expires: Some(_), .. }, .. }, .. }));
        assert!(matches!(one_stmt("paid = await paid"), StmtKind::Bind { value: Expr { kind: ExprKind::Await { expires: None, .. }, .. }, .. }));
    }

    // ── expressions ──

    #[test]
    fn operators_group_by_precedence_and_to_the_left() {
        assert_eq!(e("a + b * c"), "<a + <b * c>>");
        assert_eq!(e("a - b - c"), "<<a - b> - c>");
        assert_eq!(e("a * b / c"), "<<a * b> / c>");
        assert_eq!(e("a < b && c == d || !e"), "<<<a < b> && <c == d>> || !e>");
        assert_eq!(e("x ?? y || z"), "<x ?? <y || z>>");
        assert_eq!(e("x when a || b"), "<x when <a || b>>");
        assert_eq!(e("a ?? b ?? c"), "<<a ?? b> ?? c>");
        assert_eq!(e("(a + b) * c"), "<(<a + b>) * c>");
        assert_eq!(e("a + \n  b"), "<a + b>", "a line may continue after an operator");
        assert_eq!(e("a >= b != c <= d"), "<<a >= b> != <c <= d>>");
    }

    #[test]
    fn if_is_a_prefix_form_with_an_optional_else() {
        assert_eq!(e("if c then a else b"), "if c then a else b");
        assert_eq!(e("if c then a"), "if c then a");
        assert_eq!(e("if a when c then x ?? y else z + 1"), "if <a when c> then <x ?? y> else <z + 1>");
        assert_eq!(e("if c then\n  a\nelse\n  b"), "if c then a else b");
    }

    #[test]
    fn nouns_calls_methods_fields_and_indexes() {
        assert_eq!(e("now"), "now");
        assert_eq!(e("random"), "random");
        assert_eq!(e("ingress"), "ingress");
        assert_eq!(e("secret.api_token"), "secret.api_token");
        assert_eq!(e("state.beats"), "state.beats");
        assert_eq!(e("state.set(last_post, s)"), "state.set(last_post, s)");
        assert_eq!(e("http.post(\"hooks.slack.com\", path, headers: h, body: b)"), "http.post(\"hooks.slack.com\", path, headers: h, body: b)");
        assert_eq!(e("stripe.verify(req, secret.k, now, tolerance: 5m).event"), "stripe.verify(req, secret.k, now, tolerance: 5m).event");
        assert_eq!(e("notes.list(after: req.query.after ?? \"\", limit: 50)"), "notes.list(after: <req.query.after ?? \"\">, limit: 50)");
        assert_eq!(e("notes[req.params.id]"), "notes[req.params.id]");
        assert_eq!(e("hmac_sha256(secret.k, req.path, encoding: hex) ?? \"unsigned\""), "<hmac_sha256(secret.k, req.path, encoding: hex) ?? \"unsigned\">");
        assert_eq!(e("f(\n  a,\n  b: c,\n)"), "f(a, b: c)");
        assert_eq!(e("resume_token(paid)"), "resume_token(paid)");
        assert_eq!(e("req.request.body"), "req.request.body");
        assert_eq!(e("parse_json(page.body).slideshow.title"), "parse_json(page.body).slideshow.title");
    }

    #[test]
    fn literals_records_lists_lambdas_and_negatives() {
        assert_eq!(e("1"), "1");
        assert_eq!(e("-2.5"), "-2.5");
        assert_eq!(e("5m"), "5m");
        assert_eq!(e("true"), "true");
        assert_eq!(e("\"a\\\"b\\n\""), "\"a\\\"b\\n\"");
        assert_eq!(e("{ }"), "{}");
        assert_eq!(e("{ text: msg, \"content-type\": \"application/json\" }"), "{text: msg, content-type!: \"application/json\"}");
        assert_eq!(e("{ ..req.json, paid_via: paid.method }"), "{..req.json, paid_via: paid.method}");
        assert_eq!(e("{ ..base }"), "{..base}");
        assert_eq!(e("{\n  a: 1,\n  b: 2,\n}"), "{a: 1, b: 2}");
        assert_eq!(e("[1, \"a\", [ ]]"), "[1, \"a\", []]");
        assert_eq!(e("fn(acc, p) -> acc + p"), "fn(acc, p) -> <acc + p>");
        assert_eq!(e("map(titles, fn: slugify)"), "map(titles, fn: slugify)");
        assert_eq!(e("!done"), "!done");
    }

    #[test]
    fn strings_interpolate_with_the_columns_of_their_parts() {
        assert_eq!(e("\"Hello, {req.params.name}!\""), "\"Hello, \"+{req.params.name}+\"!\"");
        assert_eq!(e("\"{likes[id] ?? 0} likes\""), "{<likes[id] ?? 0>}+\" likes\"");
        assert_eq!(e("\"{x}\""), "{x}");
        assert_eq!(e("\"a {{b}} c\""), "\"a {b} c\"");
        assert_eq!(e("\"{ json({ a: 1 }) }\""), "{json({a: 1})}", "braces nest inside a part");
        assert_eq!(e("\"{slice(sha256(\"{now}:{x}\"), 0, 6)}!\""), "{slice(sha256({now}+\":\"+{x}), 0, 6)}+\"!\"", "a string nests inside a part, with parts of its own");
        assert_eq!(e("\"{f(\"}}\")}\""), "{f(\"}\")}", "a brace inside the nested string is its own");
        // The part's expression carries the string's columns: `x` at column 10 of `s = "ab {x}"`.
        let Item::Stmt(s) = &file("s = \"ab {x}\"").items[0] else { panic!() };
        let StmtKind::Bind { value: Expr { kind: ExprKind::Interpolated(parts), .. }, .. } = &s.kind else { panic!() };
        let Part::Expr(x) = &parts[1] else { panic!() };
        assert_eq!((x.span.start.line, x.span.start.col), (1, 10));
    }

    // ── errors ──

    #[test]
    fn errors_say_the_line_and_column_and_what_was_expected() {
        assert_eq!(error("route GET /x (req) {\n  respond 1\n"), (1, 20, "the block isn't closed with }".into()));
        assert_eq!(error("x = 1\n}\n"), (2, 1, "a } without a block to close".into()));
        assert_eq!(error("x = 1 +\n"), (2, 1, "expected an expression, found the end of the text".into()));
        // Inside parentheses a newline continues the statement, so the text's end is what's found.
        assert_eq!(error("x = (1 + 2"), (1, 11, "expected \")\", found the end of the text".into()));
        assert_eq!(error("route FETCH /x (r) {}"), (1, 7, "\"FETCH\" is not a method (GET, POST, PUT, PATCH, DELETE)".into()));
        assert_eq!(error("route GET x (r) {}"), (1, 11, "expected a route path like /todos/:id, found \"x\"".into()));
        assert_eq!(error("route GET /x {}"), (1, 14, "expected \"(\" and a name for the request, found \"{\"".into()));
        assert_eq!(error("y = secret"), (1, 5, "secret takes a name: secret.api_token".into()));
        assert_eq!(error("y = http.post"), (1, 5, "http.post is a call: http.post(…)".into()));
        assert_eq!(error("x = if c a"), (1, 10, "expected \"then\" after the condition, found \"a\"".into()));
        assert_eq!(error("f(x) = 1"), (1, 1, "can't assign to this: a name, state.key, or collection[key]".into()));
        assert_eq!(error("a.b.c -> d.e"), (1, 1, "a wire's end is node.port".into()));
        assert_eq!(error("x = -y"), (1, 5, "a minus needs a number after it (to subtract, write a - b)".into()));
        assert_eq!(error("x = f(a b)"), (1, 9, "expected \",\" or \")\" after the argument, found \"b\"".into()));
        assert_eq!(error("x = { a 1 }"), (1, 9, "expected \":\" and the field's value, found \"1\"".into()));
        assert_eq!(error("x = 1 2"), (1, 7, "unexpected \"2\" after the statement".into()));
        assert_eq!(error("x = then"), (1, 5, "expected an expression, found \"then\"".into()));
        assert_eq!(error("holon h(a -> b {}"), (1, 11, "expected \",\" or \")\" after the port, found \"->\"".into()));
        assert_eq!(error("node n = concat[x]"), (1, 17, "expected how many inputs, found \"x\"".into()));
        assert_eq!(error("every soon"), (1, 7, "expected how often, like 10s or 5m, found \"soon\"".into()));
        assert_eq!(error("paid = await paid, expire: 1d"), (1, 20, "expected \"expires\", found \"expire\"".into()));
    }

    #[test]
    fn string_errors_point_inside_the_string() {
        //    col: 123456789       the quote is column 5, `a` 6, `b` 7, the space 8, `{` 9
        assert_eq!(error("s = \"ab {x\""), (1, 9, "a { in a string isn't closed with } (write {{ for a brace)".into()));
        assert_eq!(error("s = \"ab } c\""), (1, 9, "a } in a string needs a { before it (write }} for a brace)".into()));
        assert_eq!(error("s = \"ab {}\""), (1, 9, "an empty {} in a string: put an expression inside, or write {{}} for braces".into()));
        assert_eq!(error("s = \"ab \\q\""), (1, 9, "not an escape (\\\" \\\\ \\n \\t \\uXXXX)".into()));
        assert_eq!(error("s = \"ab {x y}\""), (1, 12, "unexpected \"y\" inside {…}".into()));
        assert_eq!(error("s = \"ab {1 +}\""), (1, 13, "expected an expression, found the end of the text".into()));
    }

    // ── the samples, in both forms ──

    /// docs/LANGUAGE.md §3, the tour: every sample in the natural form.
    const NATURAL: &[(&str, &str, usize, usize)] = &[
        ("hello-api", "flow hello-api\nport 8101\n\nroute GET /hello/:name (req) {\n  respond \"Hello, {req.params.name}\"\n}\n", 1, 0),
        (
            "notes-api",
            "flow notes-api\nport 8102\ncollection notes\n\nroute GET /notes (req) {\n  respond notes.list(after: req.query.after ?? \"\", limit: 50)\n}\n\nroute PUT /notes/:id (req) {\n  notes[req.params.id] = req.json\n  respond req.json, missing: 400\n}\n\nroute GET /notes/:id (req) {\n  respond notes[req.params.id]\n}\n\nroute DELETE /notes/:id (req) {\n  delete notes[req.params.id]\n  respond 204\n}\n",
            4,
            0,
        ),
        (
            "stripe-to-slack",
            "flow stripe-to-slack\nport 8103\n\nroute POST /stripe (req) {\n  event = stripe.verify(req, secret.stripe_webhook_secret, now, tolerance: 5m).event\n  sent  = http.post(\"hooks.slack.com\", secret.slack_webhook_path,\n            headers: { \"content-type\": \"application/json\" },\n            body: json({ text: \"Stripe: {event.type}\" }))\n  respond sent.status, missing: 400\n}\n",
            1,
            0,
        ),
        (
            "guarded-api",
            "flow guarded-api\nport 8109\noptional secret.receipt_key\n\nroute GET /me (req) {\n  ok      = secure_equals(req.headers.authorization, \"Bearer {secret.api_token}\")\n  receipt = hmac_sha256(secret.receipt_key, req.path, encoding: hex) ?? \"unsigned\"\n  respond { receipt: receipt } when ok, missing: 401\n}\n",
            1,
            0,
        ),
        (
            "slugs",
            "flow slugs\nport 8110\ncollection posts\noptional secret.receipt_key\n\nholon slugify(text) -> slug {\n  slug = join(split(lowercase(text), \" \"), \"-\")\n}\n\nholon publish(slug, author) -> (put, saved, receipt) {\n  s       = slug ?? \"\"\n  put     = posts.put(s, { author: author, at: now })\n  saved   = state.set(last_post, s)\n  receipt = hmac_sha256(secret.receipt_key, s, encoding: hex) ?? \"unsigned\"\n}\n\nroute POST /slugs (req) {\n  title     = slugify(req.json.title)\n  author    = slugify(req.json.author)\n  published = publish(title, author)\n  respond { title: title, author: author, receipt: published.receipt }, missing: 400\n}\n\nroute GET /slugs (req) {\n  respond posts.list(limit: 100)\n}\n",
            2,
            2,
        ),
        ("heartbeat", "flow heartbeat\nevery 10s\n\nstate.last_beat = now\nstate.beats = (state.beats ?? 0) + 1\nstate.first_beat = state.first_beat ?? now\n", 0, 0),
        (
            "approval",
            "flow approval\nport 8105\n\nroute POST /orders (req) {\n  respond \"/paid/{resume_token(paid)}\"\n  paid = await paid, expires: 1d\n  state.last_order = { ..req.json, paid_via: paid.method }\n}\n\nroute GET /paid/:token (req) resumes paid {\n  respond \"thanks, the order is paid\"\n}\n",
            2,
            0,
        ),
        ("fetch-json", "flow fetch-json\nport 8106\n\nroute GET /slideshow (req) {\n  page = http.get(\"httpbin.org\", \"/json\")\n  respond parse_json(page.body).slideshow.title\n}\n", 1, 0),
        (
            "sign-payload",
            "flow sign-payload\nport 8107\n\nroute POST /sign (req) {\n  respond { signature:   hmac_sha256(secret.signing_key, req.body, encoding: hex),\n            body_base64: base64(req.body, mode: encode) }, missing: 400\n}\n",
            1,
            0,
        ),
        (
            "likes",
            "flow likes\nport 8108\ncollection likes\n\nroute POST /posts/:id/like (req) {\n  id = req.params.id\n  n  = (likes[id] ?? 0) + 1\n  likes[id] = n\n  respond n, missing: 400\n}\n\nroute GET /posts/:id/likes (req) {\n  respond \"{likes[req.params.id] ?? 0} likes\"\n}\n\nroute GET /likes (req) {\n  respond likes.list(limit: 100)\n}\n",
            3,
            0,
        ),
        (
            "witness",
            "flow witness\nport 8130\ncollection batches\n\nroute POST /heads (req) {\n  ok    = secure_equals(req.headers.authorization, \"Bearer {secret.witness_key}\")\n  batch = req.json.batch\n  batches[batch] = { ..req.json, witnessed_at: now } when ok\n  respond { batch: batch, witnessed_at: now } when ok, missing: 401\n}\n\nroute GET /batches (req) {\n  respond batches.list(after: req.query.after ?? \"\", limit: 200)\n}\n",
            2,
            0,
        ),
    ];

    #[test]
    fn every_sample_in_the_natural_form_parses() {
        for (name, src, routes, holons) in NATURAL {
            let f = parse(src).unwrap_or_else(|e| panic!("{name}: {e}"));
            let got_routes = f.items.iter().filter(|i| matches!(i, Item::Route(_))).count();
            let got_holons = f.items.iter().filter(|i| matches!(i, Item::Holon(_))).count();
            assert_eq!((got_routes, got_holons), (*routes, *holons), "{name}");
            assert!(matches!(&f.headers[0].kind, HeaderKind::Flow(n) if n == name), "{name}");
        }
        // The one flow without routes has its statements at the top level: three writes.
        let f = parse(NATURAL[5].1).unwrap();
        assert_eq!(f.items.len(), 3);
        assert!(f.items.iter().all(|i| matches!(i, Item::Stmt(Stmt { kind: StmtKind::StateWrite { .. }, .. }))));
    }

    /// Every sample as the wiring form prints it today, and the starter: the new parser reads the
    /// same nodes, wires and holons the old one does.
    #[test]
    fn every_sample_in_the_wiring_form_parses_as_the_old_parser_reads_it() {
        fn counts(stmts: &[Stmt]) -> (usize, usize) {
            let nodes = stmts.iter().filter(|s| matches!(s.kind, StmtKind::Node { .. })).count();
            let wires = stmts.iter().filter(|s| matches!(s.kind, StmtKind::Wire { .. })).count();
            (nodes, wires)
        }
        fn old_counts(stmts: &[text::Stmt]) -> (usize, usize) {
            let nodes = stmts.iter().filter(|s| matches!(s, text::Stmt::Node { .. })).count();
            let wires = stmts.iter().filter(|s| matches!(s, text::Stmt::Wire { .. })).count();
            (nodes, wires)
        }
        let mut texts = Vec::new();
        for s in samples::samples() {
            let doc = samples::build(s).unwrap();
            texts.push((s.name.to_string(), text::print(Some(s.name), &doc, &mut holon_core::MapResolver::new(), &crate::composites::Names::new()).unwrap()));
        }
        assert!(parse(commands::STARTER).is_ok());
        for (name, src) in texts {
            let new = parse(&src).unwrap_or_else(|e| panic!("{name}: {e}\n{src}"));
            // A js node is written as its statement in either form (the netlist has no spelling
            // for its engine), and the old parser predates it: it reads everything else.
            if new.items.iter().any(|i| matches!(i, Item::Js(_))) {
                continue;
            }
            let old = text::parse(&src).unwrap_or_else(|e| panic!("{name}: old parser: {e}"));
            let top: Vec<&Stmt> = new.items.iter().filter_map(|i| if let Item::Stmt(s) = i { Some(s) } else { None }).collect();
            let top: Vec<Stmt> = top.into_iter().cloned().collect();
            let old_top: Vec<text::Stmt> = old.body.iter().filter(|s| !matches!(s, text::Stmt::Holon { .. })).cloned().collect();
            assert_eq!(counts(&top), old_counts(&old_top), "{name}: top-level nodes and wires");
            let holons: Vec<&HolonDef> = new.items.iter().filter_map(|i| if let Item::Holon(h) = i { Some(h) } else { None }).collect();
            let old_holons: Vec<&text::Stmt> = old.body.iter().filter(|s| matches!(s, text::Stmt::Holon { .. })).collect();
            assert_eq!(holons.len(), old_holons.len(), "{name}: holons");
            for (h, o) in holons.iter().zip(old_holons) {
                let text::Stmt::Holon { name: oname, inputs, outputs, body, .. } = o else { unreachable!() };
                assert_eq!(&h.name.text, oname);
                assert_eq!(h.inputs.len(), inputs.len());
                assert_eq!(h.outputs.len(), outputs.len());
                assert_eq!(counts(&h.body), old_counts(body), "{name}: holon {oname}");
            }
            assert_eq!(new.headers.iter().filter(|h| matches!(h.kind, HeaderKind::Optional(_))).count(), old.optional.len(), "{name}: optional");
        }
    }

    #[test]
    fn a_match_has_arms_on_lines_or_after_commas_and_an_else_last() {
        assert_eq!(e("match op { \"a\" -> 1, \"b\", \"c\" -> x + 1 }"), "match op { \"a\" -> 1; \"b\", \"c\" -> <x + 1> }");
        assert_eq!(e("match req.json.op {\n  1 -> x + 1\n  -2 -> y\n  else -> 0\n}"), "match req.json.op { 1 -> <x + 1>; -2 -> y; else -> 0 }");
        assert_eq!(e("match b { true -> \"yes\", false -> \"no\", }"), "match b { true -> \"yes\"; false -> \"no\" }");
        // An atom: it goes where an expression does.
        assert_eq!(e("match op { 1 -> a } ?? 0"), "<match op { 1 -> a } ?? 0>");
        // A keyword, but a node may still be called after its leaf in the wiring form.
        assert_eq!(e("match.out"), "match.out");
    }

    #[test]
    fn a_match_says_what_is_wrong_with_its_arms() {
        assert_eq!(error("x = match op { else -> 1 }"), (1, 14, "a `match` needs an arm".into()));
        assert_eq!(error("x = match op { }"), (1, 14, "a `match` needs an arm".into()));
        assert_eq!(error("x = match op {\n  else -> 1\n  \"a\" -> 2\n}"), (3, 3, "`else` comes last, once".into()));
        assert_eq!(error("x = match op {\n  else -> 1\n  else -> 2\n}"), (3, 3, "`else` comes last, once".into()));
        assert_eq!(error("x = match op { \"a\" -> 1 \"b\" -> 2 }"), (1, 25, "expected a new line, \",\" or \"}\" after the arm, found \"\\\"b\\\"\"".into()));
        assert_eq!(error("x = match op \"a\" -> 1"), (1, 14, "expected \"{\" and the arms after the subject, found \"\\\"a\\\"\"".into()));
        assert_eq!(error("x = match op { \"a\" 1 }"), (1, 20, "expected \"->\" and the arm's value, found \"1\"".into()));
    }
}
