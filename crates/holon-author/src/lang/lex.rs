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
                    self.push(Tok::Ident, s, start);
                }
                _ => self.punct(start)?,
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

/// The tokens of `src`, the first at `start` (so a string's `{…}` part lexes with the columns of
/// the string it sits in).
pub fn lex_from(src: &str, start: Pos) -> Result<Vec<Token>, LexError> {
    Lexer { chars: src.chars().collect(), i: 0, pos: start, out: Vec::new() }.run()
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

    #[test]
    fn a_string_keeps_its_escapes_for_the_parser_and_may_hold_braces() {
        let toks = lex(r#""a \"b\" {{c}} {d}""#).unwrap();
        assert_eq!(toks[0].text, r#"a \"b\" {{c}} {d}"#);
        // An escaped quote doesn't close the string; a newline inside is an error.
        assert!(lex("\"a\nb\"").is_err());
    }
}
