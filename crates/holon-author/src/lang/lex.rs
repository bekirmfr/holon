//! Tokens of a `.holon` text, each with its span. Words are not classified here: keywords, nouns
//! and names are all `Ident`, and the parser decides by position, so `in` can head a wire and
//! `when` can be an operator without the lexer knowing which statement it is in.
//!
//! Newlines separate statements, so they are tokens (one per run of blank lines); the parser skips
//! them where a statement plainly continues: inside parentheses, brackets and record literals,
//! and after an operator or a comma. A block's braces hold statements, so the lexer can't tell a
//! newline that separates from one that continues; the parser can. `#` starts a comment to the
//! end of the line.

use super::ast::{Pos, Span};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident,
    Number,
    /// A number with a unit: `10s`, `5m`, `2h`, `1d`. The seconds it means.
    Duration(f64),
    /// A string: `text` is what is between the quotes, escapes kept for the parser.
    Str,
    /// A number with a scale: `50M`, `16MB`, `2K`. The number it means.
    Scaled(f64),
    /// A js holon's body (`holon f(a) -> b js { … }`): `text` is the JavaScript between the
    /// braces, verbatim (the braces themselves are the token's span). The lexer reads it as
    /// JavaScript, not as Holon: a `}` in a string, a template or a comment doesn't end it.
    JsBody,
    Newline,
    Eof,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Eq,
    Dot,
    DotDot,
    Arrow,
    Plus,
    Minus,
    Star,
    Slash,
    Bang,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    QQ,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub tok: Tok,
    /// The source text of the token (for a string, the inside of the quotes).
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LexError {
    pub pos: Pos,
    pub message: String,
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The seconds a duration unit stands for.
pub fn unit_seconds(unit: char) -> Option<f64> {
    Some(match unit {
        's' => 1.0,
        'm' => 60.0,
        'h' => 3600.0,
        'd' => 86400.0,
        _ => return None,
    })
}

struct Lexer {
    chars: Vec<char>,
    i: usize,
    pos: Pos,
    out: Vec<Token>,
    /// A js holon's header has been read to its `js`: its next `{` opens a JavaScript body.
    js_pending: bool,
    /// A `holon` header is being read: its parenthesis depth, and whether its `->` has come (a
    /// `js` after the outputs marks the body as JavaScript; one among the ports is a port).
    holon_head: Option<(i32, bool)>,
}

impl Lexer {
    fn peek(&self, k: usize) -> Option<char> {
        self.chars.get(self.i + k).copied()
    }

    fn bump(&mut self) -> char {
        let c = self.chars[self.i];
        self.i += 1;
        if c == '\n' {
            self.pos.line += 1;
            self.pos.col = 1;
        } else {
            self.pos.col += 1;
        }
        c
    }

    fn push(&mut self, tok: Tok, text: String, start: Pos) {
        self.out.push(Token { tok, text, span: Span { start, end: self.pos } });
    }

    fn err<T>(&self, pos: Pos, message: impl Into<String>) -> Result<T, LexError> {
        Err(LexError { pos, message: message.into() })
    }

    fn run(mut self) -> Result<Vec<Token>, LexError> {
        while let Some(c) = self.peek(0) {
            let start = self.pos;
            let before = self.out.len();
            match c {
                '#' => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.bump();
                    }
                }
                '\r' => {
                    self.bump();
                }
                '\n' => {
                    self.bump();
                    if !matches!(self.out.last(), Some(Token { tok: Tok::Newline, .. }) | None) {
                        self.push(Tok::Newline, "\n".into(), start);
                    }
                }
                c if c.is_whitespace() => {
                    self.bump();
                }
                '"' => self.string(start)?,
                c if c.is_ascii_digit() => self.number(start)?,
                c if is_ident_start(c) => {
                    let mut s = String::new();
                    while self.peek(0).is_some_and(is_ident_char) {
                        s.push(self.bump());
                    }
                    let at_start = matches!(self.out.last(), Some(Token { tok: Tok::Newline, .. }) | None);
                    if s == "holon" && at_start {
                        self.holon_head = Some((0, false));
                    } else if s == "js" && self.js_marks_body() {
                        // `holon f(a) -> b js { … }`: the block after the outputs is JavaScript.
                        self.js_pending = true;
                        self.holon_head = None;
                    } else if s == "js" && at_start && self.old_js_statement() {
                        return self.err(start, "a js node is a holon with a js body: write `holon name(inputs) -> outputs js { … }`");
                    }
                    self.push(Tok::Ident, s, start);
                }
                '{' if self.js_pending => self.js_body(start)?,
                _ => self.punct(start)?,
            }
            if self.out.len() > before {
                self.follow_holon_head();
            }
        }
        // A text always ends a statement.
        if !matches!(self.out.last(), Some(Token { tok: Tok::Newline, .. }) | None) {
            let p = self.pos;
            self.push(Tok::Newline, String::new(), p);
        }
        let p = self.pos;
        self.push(Tok::Eof, String::new(), p);
        Ok(self.out)
    }

    fn string(&mut self, start: Pos) -> Result<(), LexError> {
        self.bump(); // the opening quote
        let mut s = String::new();
        self.string_body(start, &mut s)?;
        self.push(Tok::Str, s, start);
        Ok(())
    }

    /// The characters of a string after its opening quote, up to its closing one (consumed, left
    /// out). A `{…}` part may hold a string of its own (`"{slice(sha256("{now}"), 0, 6)}"`, as
    /// the printer writes a value used once): it is scanned the same way and kept whole, quotes
    /// and all, for the part's own parse (parse.rs). Outside a part, `{{` and `}}` are braces.
    fn string_body(&mut self, start: Pos, s: &mut String) -> Result<(), LexError> {
        let mut depth = 0usize;
        loop {
            match self.peek(0) {
                None | Some('\n') => return self.err(start, "a quote isn't closed"),
                Some('"') if depth == 0 => {
                    self.bump();
                    return Ok(());
                }
                Some('"') => {
                    // A string inside the part, or the quote that ends this one with a part
                    // left open (`"ab {x"`): tried as the former, read as the latter when the
                    // text doesn't close that way (parse.rs then says which brace is open).
                    let (at, pos, len) = (self.i, self.pos, s.len());
                    s.push(self.bump());
                    if self.string_body(pos, s).is_ok() {
                        s.push('"');
                    } else {
                        (self.i, self.pos) = (at, pos);
                        s.truncate(len);
                        self.bump();
                        return Ok(());
                    }
                }
                Some('\\') => {
                    s.push(self.bump());
                    match self.peek(0) {
                        Some('\n') | None => return self.err(start, "a quote isn't closed"),
                        Some(_) => s.push(self.bump()),
                    }
                }
                Some(c @ ('{' | '}')) if depth == 0 && self.peek(1) == Some(c) => {
                    s.push(self.bump());
                    s.push(self.bump());
                }
                Some('{') => {
                    depth += 1;
                    s.push(self.bump());
                }
                Some('}') => {
                    depth = depth.saturating_sub(1);
                    s.push(self.bump());
                }
                Some(_) => s.push(self.bump()),
            }
        }
    }

    /// Whether a `js` just read marks a holon's body as JavaScript: in a holon header, after its
    /// `->` and the outputs (outside their parentheses), followed by a budget or the `{`.
    fn js_marks_body(&self) -> bool {
        let Some((0, true)) = self.holon_head else { return false };
        let after_outputs = matches!(self.out.last().map(|t| &t.tok), Some(Tok::RParen | Tok::Ident | Tok::Str | Tok::Gt));
        let mut k = 0;
        while self.peek(k).is_some_and(|c| c == ' ' || c == '\t') {
            k += 1;
        }
        after_outputs && self.peek(k).is_some_and(|c| c == '{' || is_ident_start(c))
    }

    /// `js name = …` at the start of a statement: the form a js node had before it was a holon.
    fn old_js_statement(&self) -> bool {
        let mut k = 0;
        let skip = |k: &mut usize| {
            while self.peek(*k).is_some_and(|c| c == ' ' || c == '\t') {
                *k += 1;
            }
        };
        skip(&mut k);
        if !self.peek(k).is_some_and(is_ident_start) {
            return false;
        }
        while self.peek(k).is_some_and(is_ident_char) {
            k += 1;
        }
        skip(&mut k);
        self.peek(k) == Some('=') && self.peek(k + 1) != Some('=')
    }

    /// Keeps up with a holon header as its tokens come: parentheses, the `->`, and its end at a
    /// brace (the block of a holon written in Holon) or at the end of the text.
    fn follow_holon_head(&mut self) {
        let Some((depth, arrow)) = self.holon_head.as_mut() else { return };
        match self.out.last().map(|t| &t.tok) {
            Some(Tok::LParen) => *depth += 1,
            Some(Tok::RParen) => *depth -= 1,
            Some(Tok::Arrow) if *depth == 0 => *arrow = true,
            Some(Tok::LBrace | Tok::RBrace | Tok::Eof) => self.holon_head = None,
            _ => {}
        }
    }

    /// A js body: from the `{` to its matching `}`, read as JavaScript (strings, template
    /// literals with their `${…}` parts, line and block comments), kept verbatim.
    fn js_body(&mut self, start: Pos) -> Result<(), LexError> {
        self.bump(); // the opening brace
        self.js_pending = false;
        let mut body = String::new();
        let mut depth = 1usize;
        // Template literals nest: each `${` opens a brace level that a `}` closes back into text.
        let mut templates: Vec<usize> = Vec::new();
        loop {
            let Some(c) = self.peek(0) else { return self.err(start, "the js body isn't closed: a } is missing") };
            match c {
                '}' if templates.last() == Some(&depth) => {
                    // The end of a `${…}` part: back into the template's text.
                    templates.pop();
                    depth -= 1;
                    body.push(self.bump());
                    self.js_template(start, &mut body, &mut templates, &mut depth)?;
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        self.bump();
                        self.push(Tok::JsBody, body, start);
                        return Ok(());
                    }
                    body.push(self.bump());
                }
                '{' => {
                    depth += 1;
                    body.push(self.bump());
                }
                '\'' | '"' => {
                    let quote = self.bump();
                    body.push(quote);
                    loop {
                        match self.peek(0) {
                            None | Some('\n') => return self.err(start, "a string in the js body isn't closed"),
                            Some('\\') => {
                                body.push(self.bump());
                                if self.peek(0).is_some() {
                                    body.push(self.bump());
                                }
                            }
                            Some(c) if c == quote => {
                                body.push(self.bump());
                                break;
                            }
                            Some(_) => body.push(self.bump()),
                        }
                    }
                }
                '`' => {
                    body.push(self.bump());
                    self.js_template(start, &mut body, &mut templates, &mut depth)?;
                }
                '/' if self.peek(1) == Some('/') => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        body.push(self.bump());
                    }
                }
                // A regex literal (`/[&<>"']/g`): quotes and braces inside it are its own. A `/`
                // is one where a value can't be, after an operator, an opening bracket or a
                // keyword such as `return`; after a value it divides.
                '/' if regex_may_start(&body) => {
                    body.push(self.bump());
                    let mut in_class = false;
                    loop {
                        match self.peek(0) {
                            None | Some('\n') => return self.err(start, "a regex in the js body isn't closed"),
                            Some('\\') => {
                                body.push(self.bump());
                                if self.peek(0).is_some_and(|c| c != '\n') {
                                    body.push(self.bump());
                                }
                            }
                            Some('[') => {
                                in_class = true;
                                body.push(self.bump());
                            }
                            Some(']') => {
                                in_class = false;
                                body.push(self.bump());
                            }
                            Some('/') if !in_class => {
                                body.push(self.bump());
                                while self.peek(0).is_some_and(|c| c.is_ascii_alphabetic()) {
                                    body.push(self.bump());
                                }
                                break;
                            }
                            Some(_) => body.push(self.bump()),
                        }
                    }
                }
                '/' if self.peek(1) == Some('*') => {
                    body.push(self.bump());
                    body.push(self.bump());
                    loop {
                        match self.peek(0) {
                            None => return self.err(start, "a comment in the js body isn't closed"),
                            Some('*') if self.peek(1) == Some('/') => {
                                body.push(self.bump());
                                body.push(self.bump());
                                break;
                            }
                            Some(_) => body.push(self.bump()),
                        }
                    }
                }
                _ => body.push(self.bump()),
            }
        }
    }

    /// The text of a template literal after its opening backtick (or after a `}` that ended a
    /// part), up to its closing backtick or the next `${`, which opens a part the body reads as
    /// code until the `}` at this depth.
    fn js_template(&mut self, start: Pos, body: &mut String, templates: &mut Vec<usize>, depth: &mut usize) -> Result<(), LexError> {
        loop {
            match self.peek(0) {
                None => return self.err(start, "a template literal in the js body isn't closed"),
                Some('\\') => {
                    body.push(self.bump());
                    if self.peek(0).is_some() {
                        body.push(self.bump());
                    }
                }
                Some('`') => {
                    body.push(self.bump());
                    return Ok(());
                }
                Some('$') if self.peek(1) == Some('{') => {
                    body.push(self.bump());
                    body.push(self.bump());
                    *depth += 1;
                    templates.push(*depth);
                    return Ok(());
                }
                Some(_) => body.push(self.bump()),
            }
        }
    }

    fn number(&mut self, start: Pos) -> Result<(), LexError> {
        // A unit's hash, as the wiring form writes it bare (`fn: 0068ce…`): a word, not a number.
        let hex = (0..64).all(|k| self.peek(k).is_some_and(|c| matches!(c, '0'..='9' | 'a'..='f')));
        if hex && !self.peek(64).is_some_and(is_ident_char) && (0..64).any(|k| self.peek(k).is_some_and(|c| c.is_ascii_alphabetic())) {
            let word: String = (0..64).map(|_| self.bump()).collect();
            self.push(Tok::Ident, word, start);
            return Ok(());
        }
        let mut s = String::new();
        while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            s.push(self.bump());
        }
        if self.peek(0) == Some('.') && self.peek(1).is_some_and(|c| c.is_ascii_digit()) {
            s.push(self.bump());
            while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                s.push(self.bump());
            }
        }
        if matches!(self.peek(0), Some('e' | 'E')) {
            let sign = matches!(self.peek(1), Some('+' | '-'));
            let digit_at = if sign { 2 } else { 1 };
            if self.peek(digit_at).is_some_and(|c| c.is_ascii_digit()) {
                s.push(self.bump());
                if sign {
                    s.push(self.bump());
                }
                while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                    s.push(self.bump());
                }
            }
        }
        // A scale makes it a budget: `50M`, `2K`, `16MB`, `1GB`.
        if let Some(scale) = self.peek(0).filter(|c| matches!(c, 'K' | 'M' | 'G')) {
            let bytes = self.peek(1) == Some('B');
            let after = if bytes { 2 } else { 1 };
            if !self.peek(after).is_some_and(is_ident_char) {
                let n: f64 = s.parse().map_err(|_| LexError { pos: start, message: format!("{s} is not a number") })?;
                let per = match scale {
                    'K' => 1_000.0,
                    'M' => 1_000_000.0,
                    _ => 1_000_000_000.0,
                };
                let per = if bytes { per / 1_000.0 * 1_024.0 * (if scale == 'K' { 1.0 } else if scale == 'M' { 1_024.0 / 1_000.0 } else { 1_024.0 * 1_024.0 / 1_000_000.0 }) } else { per };
                for _ in 0..after {
                    s.push(self.bump());
                }
                self.push(Tok::Scaled(n * per), s, start);
                return Ok(());
            }
        }
        // A unit makes it a duration; any other letter is a mistake.
        if let Some(u) = self.peek(0).filter(|c| is_ident_char(*c)) {
            match unit_seconds(u) {
                Some(per) if !self.peek(1).is_some_and(is_ident_char) => {
                    let n: f64 = s.parse().map_err(|_| LexError { pos: start, message: format!("{s} is not a number") })?;
                    s.push(self.bump());
                    self.push(Tok::Duration(n * per), s, start);
                    return Ok(());
                }
                _ => {
                    let mut word = s.clone();
                    while self.peek(0).is_some_and(is_ident_char) {
                        word.push(self.bump());
                    }
                    return self.err(start, format!("\"{word}\" is not a number (a duration is a number then s, m, h or d: 10s, 5m)"));
                }
            }
        }
        self.push(Tok::Number, s, start);
        Ok(())
    }

    fn punct(&mut self, start: Pos) -> Result<(), LexError> {
        let two = |a: char, b: char, l: &Self| l.peek(0) == Some(a) && l.peek(1) == Some(b);
        let (tok, n) = if two('-', '>', self) {
            (Tok::Arrow, 2)
        } else if two('.', '.', self) {
            (Tok::DotDot, 2)
        } else if two('=', '=', self) {
            (Tok::EqEq, 2)
        } else if two('!', '=', self) {
            (Tok::NotEq, 2)
        } else if two('<', '=', self) {
            (Tok::Le, 2)
        } else if two('>', '=', self) {
            (Tok::Ge, 2)
        } else if two('&', '&', self) {
            (Tok::AndAnd, 2)
        } else if two('|', '|', self) {
            (Tok::OrOr, 2)
        } else if two('?', '?', self) {
            (Tok::QQ, 2)
        } else {
            let tok = match self.peek(0).expect("a char") {
                '(' => Tok::LParen,
                ')' => Tok::RParen,
                '{' => Tok::LBrace,
                '}' => Tok::RBrace,
                '[' => Tok::LBracket,
                ']' => Tok::RBracket,
                ',' => Tok::Comma,
                ':' => Tok::Colon,
                '=' => Tok::Eq,
                '.' => Tok::Dot,
                '+' => Tok::Plus,
                '-' => Tok::Minus,
                '*' => Tok::Star,
                '/' => Tok::Slash,
                '!' => Tok::Bang,
                '<' => Tok::Lt,
                '>' => Tok::Gt,
                c => return self.err(start, format!("unexpected character {c:?}")),
            };
            (tok, 1)
        };
        let mut text = String::new();
        for _ in 0..n {
            text.push(self.bump());
        }
        self.push(tok, text, start);
        Ok(())
    }
}

/// Whether a `/` after `before` (the js body so far) starts a regex literal rather than dividing:
/// at the start, after an operator or an opening bracket, or after a keyword a value follows.
fn regex_may_start(before: &str) -> bool {
    let trimmed = before.trim_end();
    let Some(last) = trimmed.chars().last() else { return true };
    if "(,=:[!&|?{};+-*%<>~^".contains(last) {
        return true;
    }
    let word: String = trimmed.chars().rev().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$').collect::<Vec<_>>().into_iter().rev().collect();
    matches!(word.as_str(), "return" | "typeof" | "case" | "in" | "of" | "delete" | "void" | "throw" | "new" | "instanceof" | "yield" | "await" | "else" | "do")
}

/// The tokens of `src`, the first at `start` (so a string's `{…}` part lexes with the columns of
/// the string it sits in).
pub fn lex_from(src: &str, start: Pos) -> Result<Vec<Token>, LexError> {
    Lexer { chars: src.chars().collect(), i: 0, pos: start, out: Vec::new(), js_pending: false, holon_head: None }.run()
}

/// The tokens of a whole text.
pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    lex_from(src, Pos { line: 1, col: 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<Tok> {
        lex(src).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn words_numbers_durations_strings_and_punctuation() {
        let toks = lex("x = 10s + 2.5e3 ?? \"a {b}\" -> ..").unwrap();
        let got: Vec<(Tok, &str)> = toks.iter().map(|t| (t.tok.clone(), t.text.as_str())).collect();
        assert_eq!(
            got,
            [
                (Tok::Ident, "x"),
                (Tok::Eq, "="),
                (Tok::Duration(10.0), "10s"),
                (Tok::Plus, "+"),
                (Tok::Number, "2.5e3"),
                (Tok::QQ, "??"),
                (Tok::Str, "a {b}"),
                (Tok::Arrow, "->"),
                (Tok::DotDot, ".."),
                (Tok::Newline, ""),
                (Tok::Eof, ""),
            ]
        );
        assert_eq!(kinds("5m 2h 1d"), [Tok::Duration(300.0), Tok::Duration(7200.0), Tok::Duration(86400.0), Tok::Newline, Tok::Eof]);
    }

    /// A string inside a `{…}` part is part of the string, quotes and all, as are the parts and
    /// braces inside it; outside a part a quote still ends the string.
    #[test]
    fn a_string_inside_a_part_belongs_to_the_string() {
        let texts = |src: &str| lex(src).unwrap().into_iter().filter(|t| t.tok == Tok::Str).map(|t| t.text).collect::<Vec<_>>();
        assert_eq!(texts("s = \"{f(\"x\")}-{g(\"{{\")} {{\" + \"}}\""), ["{f(\"x\")}-{g(\"{{\")} {{", "}}"]);
        assert_eq!(texts("s = \"a {slice(sha256(\"{now}:{x}\"), 0, 6)} b\""), ["a {slice(sha256(\"{now}:{x}\"), 0, 6)} b"]);
        assert_eq!(texts("s = \"{h(\"q \\\" q\")}\""), ["{h(\"q \\\" q\")}"], "an escaped quote inside stays escaped");
        assert!(lex("s = \"{f(\"x\")\n").is_err(), "a string inside an open part doesn't close the outer one");
    }

    #[test]
    fn spans_count_lines_and_columns_in_characters() {
        let toks = lex("ab = \"é\"\n  cd").unwrap();
        let at = |i: usize| (toks[i].span.start.line, toks[i].span.start.col, toks[i].span.end.col);
        assert_eq!(at(0), (1, 1, 3));
        assert_eq!(at(2), (1, 6, 9), "the string spans its quotes; é is one column");
        assert_eq!(toks[3].tok, Tok::Newline);
        assert_eq!(at(4), (2, 3, 5));
    }

    #[test]
    fn newlines_are_one_token_per_run_and_the_text_ends_with_one() {
        assert_eq!(kinds("a\n\n\nb"), [Tok::Ident, Tok::Newline, Tok::Ident, Tok::Newline, Tok::Eof]);
        // Inside brackets too: the parser skips them where a statement continues.
        assert_eq!(
            kinds("f(\n a,\n b\n)"),
            [Tok::Ident, Tok::LParen, Tok::Newline, Tok::Ident, Tok::Comma, Tok::Newline, Tok::Ident, Tok::Newline, Tok::RParen, Tok::Newline, Tok::Eof]
        );
        assert_eq!(kinds("\n\n# only a comment\n"), [Tok::Eof]);
        assert_eq!(kinds("a # trailing\r\nb"), [Tok::Ident, Tok::Newline, Tok::Ident, Tok::Newline, Tok::Eof]);
    }

    #[test]
    fn errors_point_at_the_column() {
        let e = lex("x = \"open").unwrap_err();
        assert_eq!((e.pos.line, e.pos.col), (1, 5));
        assert_eq!(e.message, "a quote isn't closed");
        let e = lex("y = 10sec").unwrap_err();
        assert_eq!((e.pos.line, e.pos.col), (1, 5));
        assert!(e.message.starts_with("\"10sec\" is not a number"));
        // A hash that starts with a digit is a word (the wiring form writes them bare); one
        // character short, it is a bad number again.
        let hash = format!("0068ce{}", "a".repeat(58));
        assert_eq!(kinds(&format!("fn: {hash}")), [Tok::Ident, Tok::Colon, Tok::Ident, Tok::Newline, Tok::Eof]);
        assert!(lex(&format!("fn: {}", &hash[..63])).unwrap_err().message.starts_with("\"0068ce"));
        let e = lex("a\n  b @ c").unwrap_err();
        assert_eq!((e.pos.line, e.pos.col), (2, 5));
        assert_eq!(e.message, "unexpected character '@'");
    }

    /// A js body is read as JavaScript: a `}` in a string, a template part, a comment or a regex
    /// (quotes inside one included) doesn't end it, and a `/` after a value divides.
    #[test]
    fn a_js_body_is_read_as_javascript() {
        let body = |src: &str| lex(src).unwrap().into_iter().find(|t| t.tok == Tok::JsBody).map(|t| t.text).unwrap();
        assert_eq!(body("holon f(a) -> b js { return a / 2 }"), " return a / 2 ");
        assert_eq!(body("holon f(s) -> t js { return s.replace(/[&<>\"']}/g, \"x\") }"), " return s.replace(/[&<>\"']}/g, \"x\") ");
        assert_eq!(body("holon f(s) -> t js { const r = /\\/{/; return `${s}}` + '}' // }\n}"), " const r = /\\/{/; return `${s}}` + '}' // }\n");
        assert_eq!(body("holon f(a, b) -> (c, d) js fuel 50M { return (a + 1) / (b - 1) }"), " return (a + 1) / (b - 1) ");
        assert_eq!(body("holon f(\n  a: Text,\n  b,\n) -> c: List<Text> js { return [a] }"), " return [a] ");
        assert!(lex("holon f(s) -> t js { return /[/ }\n").is_err(), "an open regex is a mistake");
    }

    /// Only a `js` after a holon's outputs marks its body: a port called `js` is a port, and a
    /// holon written in Holon has a Holon block.
    #[test]
    fn js_marks_a_body_only_after_the_outputs() {
        let has_body = |src: &str| lex(src).unwrap().iter().any(|t| t.tok == Tok::JsBody);
        assert!(!has_body("holon f(js) -> y {\n  y = js\n}\n"));
        assert!(!has_body("holon f(a) -> js {\n  js = a\n}\n"));
        assert!(!has_body("holon f(a) -> (b, js) {\n  b = a\n  js = a\n}\n"));
        assert!(!has_body("js = 1\n"));
        // The old statement says how it is written now.
        let e = lex("js f = (a) -> b { return a }\n").unwrap_err();
        assert_eq!((e.pos.line, e.pos.col), (1, 1));
        assert!(e.message.contains("holon name(inputs) -> outputs js"), "{}", e.message);
    }

    #[test]
    fn a_string_keeps_its_escapes_for_the_parser_and_may_hold_braces() {
        let toks = lex(r#""a \"b\" {{c}} {d}""#).unwrap();
        assert_eq!(toks[0].text, r#"a \"b\" {{c}} {d}"#);
        // An escaped quote doesn't close the string; a newline inside is an error.
        assert!(lex("\"a\nb\"").is_err());
    }
}
