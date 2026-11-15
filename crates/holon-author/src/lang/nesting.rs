//! How deep a text nests, counted before anything parses it. The parser (oxc's), the reader, the
//! lowering, the formatter and every walk over the tree each go a call deeper for each level a
//! text nests, and none of them stops: a text nested deep enough, a thousand parentheses, overflows
//! the stack of the program reading it, which ends without a word, be it `holon check`, `holon
//! lsp`, the MCP server answering an agent's `check`, or the editor's wasm. So a text is counted
//! first, by a scanner that never recurses, and one that nests past [`MAX_NESTING`] is refused at
//! the place it does, before anything else reads it.
//!
//! A level is a step down the text's syntax tree. Each bracket opens one, `(`, `[`, `{` and a
//! template's `${`, and so does each operator or keyword that puts what follows it under what came
//! before it in the same statement: `a + b`, `!x`, `x => …`, `c ? a : b`, `a.b`, `new X`, `do …`.
//! A chain the parser reads in a loop counts as deep as its tree is (`a + b + c` is two levels, as
//! `(a + b) + c`). What follows a `,` or a `;` is a sibling of what came before it and starts
//! again, as does a statement begun on a line of its own; a string, a comment and a template's
//! text count nothing. The count is the tree's depth or more, never less, for any text TypeScript
//! reads: a statement is only taken to end where TypeScript ends it.
//!
//! So the scanner reads a text's words, comments and line ends as oxc's lexer does, or a text
//! written to be read one way by it and another by the parser gets past (audit R36): a line ends
//! at `\n`, `\r`, U+2028 or U+2029, which end a `//` comment, a string and a regular expression;
//! the spaces are oxc's (U+00A0, U+FEFF, U+3000 and the rest, never a word); `#!` on the first
//! line is a comment; a `\` before a line's end goes on with a string. Where TypeScript's own
//! reading of a `/` hangs on what the parser makes of what came before it, after a `}`, an `of`
//! or a `>` that may close a type's arguments, the text is refused at it, to be written in
//! parentheses: one read as a division and the other as a regular expression would each skip what
//! the other counts. A word that wants what follows it (`as`, `keyof`, `in`) keeps its statement
//! open over a line's end, and an `else` after a statement that ended goes on its `if`: `if (a)
//! x; else if (b) y; else …` nests as deep as its links.

use std::cell::Cell;

use super::ast::{Pos, Span};
use super::lower::LowerError;

/// How deep a text may nest, in levels of its syntax tree (the module's doc). A flow nests a few
/// dozen levels: the samples' deepest is chess's, 33. The costliest form this deep, `f(f(…))`,
/// takes a third of the stack the editor's wasm has, in its release build (it traps at 415).
pub const MAX_NESTING: usize = 128;

/// The stack a text is read on: a level of the costliest form takes tens of KiB of it in a debug
/// build (a few in release), so [`MAX_NESTING`] of them fit several times over.
const DEEP_STACK: usize = 64 << 20;

thread_local! {
    /// Whether this thread is one [`on_deep_stack`] started.
    static DEEP: Cell<bool> = const { Cell::new(false) };
}

/// `f`, which reads a text counted already, run on a thread of its own with [`DEEP_STACK`],
/// whatever thread asks: the command line's main thread has 1 MiB on Windows, an MCP call's 2 MiB.
/// On such a thread already, it runs there. wasm has no threads: there it runs on the module's
/// stack, in a release build.
pub(crate) fn on_deep_stack<T: Send>(f: impl FnOnce() -> Result<T, LowerError> + Send) -> Result<T, LowerError> {
    if cfg!(target_arch = "wasm32") || DEEP.with(Cell::get) {
        return f();
    }
    std::thread::scope(|s| {
        let read = std::thread::Builder::new().name("holon text".into()).stack_size(DEEP_STACK).spawn_scoped(s, || {
            DEEP.with(|deep| deep.set(true));
            f()
        });
        match read {
            Ok(reading) => reading.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            Err(e) => Err(LowerError { span: Span::default(), message: format!("no thread to read the text on ({e})") }),
        }
    })
}

/// Why a text is refused before it is read, at a byte.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Refused {
    /// It nests past the limit here.
    Deep(usize),
    /// A `/` here reads as a division or a regular expression as the parser takes what came
    /// before it (the module's doc).
    TwoWays(usize),
}

impl Refused {
    fn at(self) -> usize {
        match self {
            Refused::Deep(at) | Refused::TwoWays(at) => at,
        }
    }
}

/// `src` refused at the place it nests past [`MAX_NESTING`], if it does, or where a `/` in it
/// reads two ways.
pub fn check(src: &str) -> Result<(), LowerError> {
    match deepest(src, MAX_NESTING) {
        Ok(_) => Ok(()),
        Err(refused) => {
            let at = pos(src.as_bytes(), refused.at());
            let message = match refused {
                Refused::Deep(_) => format!("this nests more than {MAX_NESTING} levels deep: name a part of it, with a const or a function"),
                Refused::TwoWays(_) => "this `/` reads two ways in TypeScript, after a `}`, an `of` or a `>` that may close a type's arguments: a division, or a regular expression's start; put the value it divides, or the regular expression, in parentheses".to_string(),
            };
            Err(LowerError { span: Span { start: at, end: at }, message })
        }
    }
}

/// Where `src` nests deepest, the first place it does.
pub(crate) fn deepest_at(src: &str) -> Pos {
    let most = deepest(src, usize::MAX).unwrap_or_default();
    pos(src.as_bytes(), deepest(src, most.saturating_sub(1)).err().map(Refused::at).unwrap_or_default())
}

/// The line and column (in characters) of byte `at`.
pub(crate) fn pos(b: &[u8], at: usize) -> Pos {
    let before = &b[..at.min(b.len())];
    let start = before.iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1);
    let line = before.iter().filter(|&&c| c == b'\n').count();
    // A character is its first byte: those that aren't UTF-8's continuation bytes.
    let col = before[start..].iter().filter(|&&c| c & 0xC0 != 0x80).count();
    Pos { line: line as u32 + 1, col: col as u32 + 1 }
}

/// The operators and punctuation of more than one character, the longest first.
const PUNCTUATION: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=", "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
    "**", "<<", ">>",
];

/// The words that put what follows them under them: `typeof x`, `new X`, `a in b`, `do …`. Each
/// wants what follows it, so a line's end after one doesn't end its statement (`x as⏎any`).
const NESTING_WORDS: &[&str] = &["typeof", "void", "delete", "await", "yield", "new", "do", "in", "instanceof", "of", "as", "satisfies", "keyof", "infer", "extends", "unique", "asserts", "is", "readonly"];

/// The words after which a value is still to come, so a `/` starts a regular expression.
const BEFORE_A_VALUE: &[&str] = &["return", "typeof", "case", "in", "of", "delete", "void", "throw", "new", "else", "do", "await", "yield", "instanceof", "extends"];

/// The words that go on a statement begun on a line before: `a\nin b`, `} else {`, `} catch {`.
const GOING_ON: &[&str] = &["in", "instanceof", "as", "satisfies", "of", "extends", "implements", "is", "else", "catch", "finally", "while"];

/// The space or the line's end at byte `i`, as oxc's lexer reads them: its length, and whether
/// it ends a line. A space is a tab, a vertical tab, a form feed, a space, or one of Unicode's
/// that oxc takes as one (NEL, NBSP, U+1680, U+2000 to U+200B, U+202F, U+205F, U+3000, U+FEFF);
/// a line ends at `\n`, `\r`, U+2028 and U+2029.
fn space_at(b: &[u8], i: usize) -> Option<(usize, bool)> {
    match *b.get(i..)? {
        [b'\n' | b'\r', ..] => Some((1, true)),
        [b' ' | b'\t' | 0x0B | 0x0C, ..] => Some((1, false)),
        [0xE2, 0x80, 0xA8 | 0xA9, ..] => Some((3, true)),
        [0xC2, 0x85 | 0xA0, ..] => Some((2, false)),
        [0xE1, 0x9A, 0x80, ..] | [0xE2, 0x80, 0x80..=0x8B | 0xAF, ..] | [0xE2, 0x81, 0x9F, ..] | [0xE3, 0x80, 0x80, ..] | [0xEF, 0xBB, 0xBF, ..] => Some((3, false)),
        _ => None,
    }
}

/// Whether a line ends at byte `i`.
fn line_ends_at(b: &[u8], i: usize) -> bool {
    space_at(b, i).is_some_and(|(_, ends)| ends)
}

/// A bracket's level, or the text's top.
struct Level {
    /// What opened it: `(`, `[`, `{`, `$` for a template's `${`, `` ` `` for a template's text, or
    /// nothing at the top.
    open: u8,
    /// The levels counted in it since its list or its statement began.
    count: usize,
    /// The `<`s in it not closed yet: a `,` among them separates type arguments, not siblings.
    angles: usize,
    /// Whether it is the head of an `if`, a `while`, a `for` or a `with`, which a statement
    /// follows: a value is still to come after its `)`.
    head: bool,
    /// The `else`s of the chain going on in it, each a level of the `if` it goes on: kept over
    /// the statements that end between them (`if (a) x; else if (b) y; else …`).
    elses: usize,
    /// Whether a statement or a list's item ended in it with the last token (a `;`, a `,`, a
    /// line): an `else` now goes on the chain; anything else ends it.
    ended: bool,
}

impl Level {
    fn new(open: u8, head: bool) -> Level {
        Level { open, count: 0, angles: 0, head, elses: 0, ended: false }
    }
}

/// What the last token was, as what may follow it.
#[derive(Clone, Copy, PartialEq)]
enum Last {
    /// A value's end: a `/` after it divides, and a new line after it may end the statement.
    Value,
    /// Anything else: an operator, an opening bracket, a keyword a value follows, the text's start.
    Other,
}

struct Count<'s> {
    src: &'s str,
    b: &'s [u8],
    i: usize,
    levels: Vec<Level>,
    /// The sum of the levels' counts and elses: how deep the token being read is.
    depth: usize,
    /// The deepest so far, and how deep it may go.
    deepest: usize,
    limit: usize,
    last: Last,
    /// The last two tokens, for what a `(` heads.
    words: [&'s str; 2],
    /// Whether a line ended since the last token.
    newline: bool,
    /// Whether the last token was a word that wants what follows it ([`NESTING_WORDS`]).
    hangs: bool,
    /// Whether a `/` next reads two ways: after a `}` that closed a `{`, an `of`, or a `>` that
    /// may close a type's arguments.
    two_ways: bool,
}

/// How deep `src` nests, or where it is refused: the byte where it first nests past `limit`, or
/// a `/` that reads two ways.
pub(crate) fn deepest(src: &str, limit: usize) -> Result<usize, Refused> {
    let root = Level::new(0, false);
    let mut c = Count { src, b: src.as_bytes(), i: 0, levels: vec![root], depth: 0, deepest: 0, limit, last: Last::Other, words: ["", ""], newline: false, hangs: false, two_ways: false };
    // `#!` at the very start is a comment to its line's end, as the lexer's first token.
    if c.b.starts_with(b"#!") {
        c.skip_to_line_end();
    }
    while c.i < c.b.len() {
        if c.top().open == b'`' {
            c.template_text()?;
        } else {
            c.code()?;
        }
    }
    Ok(c.deepest)
}

impl<'s> Count<'s> {
    fn top(&mut self) -> &mut Level {
        self.levels.last_mut().expect("the top level stays")
    }

    fn peek(&self, n: usize) -> Option<u8> {
        self.b.get(self.i + n).copied()
    }

    /// To the line's end, not taking it.
    fn skip_to_line_end(&mut self) {
        while self.i < self.b.len() && !line_ends_at(self.b, self.i) {
            self.i += 1;
        }
    }

    /// One level deeper, in the level the token at `at` is in.
    fn deeper(&mut self, at: usize) -> Result<(), Refused> {
        self.top().count += 1;
        self.depth += 1;
        self.deepest = self.deepest.max(self.depth);
        if self.depth > self.limit { Err(Refused::Deep(at)) } else { Ok(()) }
    }

    /// What comes next is a sibling of what came before it in this level.
    fn sibling(&mut self) {
        let level = self.top();
        let count = std::mem::take(&mut level.count);
        level.ended = true;
        self.depth -= count;
    }

    fn open(&mut self, open: u8, head: bool) {
        self.levels.push(Level::new(open, head));
    }

    /// The level closed: the one under it is the token's.
    fn close(&mut self) -> Option<Level> {
        if self.levels.len() == 1 {
            return None;
        }
        let level = self.levels.pop()?;
        self.depth -= level.count + level.elses;
        Some(level)
    }

    fn said(&mut self, token: &'s str, last: Last) {
        self.words = [self.words[1], token];
        self.last = last;
    }

    /// A template's text, to its end or its next `${`.
    fn template_text(&mut self) -> Result<(), Refused> {
        while let Some(c) = self.peek(0) {
            match c {
                b'`' => {
                    self.i += 1;
                    self.close();
                    self.said("`", Last::Value);
                    return Ok(());
                }
                b'\\' => self.i += 2,
                b'$' if self.peek(1) == Some(b'{') => {
                    // Each `${…}` is a sibling of the template's others.
                    let at = self.i;
                    self.i += 2;
                    self.sibling();
                    self.deeper(at)?;
                    self.open(b'$', false);
                    self.said("${", Last::Other);
                    self.newline = false;
                    return Ok(());
                }
                _ => self.i += 1,
            }
        }
        Ok(())
    }

    /// Code: a space, a line's end or a comment, or one token.
    fn code(&mut self) -> Result<(), Refused> {
        if let Some((len, ends)) = space_at(self.b, self.i) {
            self.newline |= ends;
            self.i += len;
            return Ok(());
        }
        match self.b[self.i] {
            b'/' if self.peek(1) == Some(b'/') => self.skip_to_line_end(),
            b'/' if self.peek(1) == Some(b'*') => {
                self.i += 2;
                while self.i < self.b.len() {
                    if self.b[self.i] == b'*' && self.peek(1) == Some(b'/') {
                        self.i += 2;
                        break;
                    }
                    // A comment over lines ends a line, as one does for ending a statement.
                    self.newline |= line_ends_at(self.b, self.i);
                    self.i += 1;
                }
            }
            _ => {
                if self.newline && self.last == Last::Value && !self.hangs && self.starts_statement() {
                    self.sibling();
                }
                // An `else` goes on its `if` whatever ended before it; anything else after a
                // statement that ended ends the chain of them.
                let else_here = !matches!(self.words[1], "." | "?.") && is_word_start(self.b[self.i]) && self.word_here() == "else";
                let level = self.top();
                if std::mem::take(&mut level.ended) && !else_here {
                    let elses = std::mem::take(&mut level.elses);
                    self.depth -= elses;
                }
                let line_start = std::mem::take(&mut self.newline);
                self.token(line_start)?;
            }
        }
        Ok(())
    }

    /// Whether the token here, first on its line after a value, can only start a statement: a
    /// word that doesn't go on the one before (`else`, `in`), a literal, `!`, `~`. Anything else
    /// may go on it (`(`, `[`, `.`, `+`, a template), so the statement is taken to go on.
    fn starts_statement(&self) -> bool {
        match self.b[self.i] {
            b'\'' | b'"' | b'~' | b'@' | b'#' | b'0'..=b'9' => true,
            b'!' => self.peek(1) != Some(b'='),
            c if is_word_start(c) => !GOING_ON.contains(&self.word_here()),
            _ => false,
        }
    }

    /// The word here: to its first byte that is no word's, or a space oxc takes as one.
    fn word_here(&self) -> &'s str {
        let mut end = self.i;
        while end < self.b.len() && is_word_part(self.b[end]) && space_at(self.b, end).is_none() {
            end += 1;
        }
        &self.src[self.i..end]
    }

    /// One token, `line_start` when a line ended before it.
    fn token(&mut self, line_start: bool) -> Result<(), Refused> {
        let at = self.i;
        let c = self.b[at];
        let two_ways = std::mem::take(&mut self.two_ways);
        self.hangs = false;
        if is_word_start(c) {
            let word = self.word_here();
            self.i += word.len();
            // After a `.`, a word is a property's name, whatever it is.
            let property = matches!(self.words[1], "." | "?.");
            if !property && word == "else" {
                // A level of the `if` it goes on, kept while the chain does: a `;` before the
                // next `else` doesn't end it (`if (a) x; else if (b) y; else …`).
                self.top().elses += 1;
                self.depth += 1;
                self.deepest = self.deepest.max(self.depth);
                if self.depth > self.limit {
                    return Err(Refused::Deep(at));
                }
            }
            if !property && NESTING_WORDS.contains(&word) {
                self.deeper(at)?;
                self.hangs = true;
            }
            self.two_ways = !property && word == "of";
            let last = if !property && BEFORE_A_VALUE.contains(&word) { Last::Other } else { Last::Value };
            self.said(word, last);
        } else if c.is_ascii_digit() || (c == b'.' && self.peek(1).is_some_and(|d| d.is_ascii_digit())) {
            self.number();
            self.said("", Last::Value);
        } else if c == b'\'' || c == b'"' {
            self.string(c);
            self.said("", Last::Value);
        } else if c == b'`' {
            self.i += 1;
            self.open(b'`', false);
        } else if c == b'/' && two_ways {
            return Err(Refused::TwoWays(at));
        } else if c == b'/' && self.last != Last::Value {
            self.regex();
            self.said("", Last::Value);
        } else {
            self.punctuation(at, line_start)?;
        }
        Ok(())
    }

    /// `86400`, `2.5`, `1e-3`, `1_000_000`, `0xff`, `10n`.
    fn number(&mut self) {
        let hex = self.peek(0) == Some(b'0') && matches!(self.peek(1), Some(b'x' | b'X'));
        while let Some(c) = self.peek(0) {
            let sign = !hex && matches!(c, b'+' | b'-') && matches!(self.b[self.i - 1], b'e' | b'E');
            if !(c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || sign) {
                break;
            }
            self.i += 1;
        }
    }

    /// A string, to its closing quote or its line's end at `\n` or `\r` (a mistake the lexer
    /// stops at, as here); a `\` before a line's end goes on with it.
    fn string(&mut self, quote: u8) {
        self.i += 1;
        while let Some(c) = self.peek(0) {
            match c {
                b'\\' if self.peek(1) == Some(b'\r') && self.peek(2) == Some(b'\n') => self.i += 3,
                b'\\' => self.i += 2,
                b'\n' | b'\r' => return,
                _ if c == quote => {
                    self.i += 1;
                    return;
                }
                _ => self.i += 1,
            }
        }
    }

    /// A regular expression, with its flags, to its closing `/` or its line's end (a `\` before
    /// it too: the lexer stops there).
    fn regex(&mut self) {
        self.i += 1;
        let mut class = false;
        while self.i < self.b.len() && !line_ends_at(self.b, self.i) {
            match self.b[self.i] {
                b'\\' if line_ends_at(self.b, self.i + 1) => self.i += 1,
                b'\\' => self.i += 2,
                b'/' if !class => {
                    self.i += 1;
                    break;
                }
                c => {
                    class = (class || c == b'[') && c != b']';
                    self.i += 1;
                }
            }
        }
        while self.i < self.b.len() && is_word_part(self.b[self.i]) && space_at(self.b, self.i).is_none() {
            self.i += 1;
        }
    }

    fn punctuation(&mut self, at: usize, line_start: bool) -> Result<(), Refused> {
        let p = PUNCTUATION.iter().find(|p| self.b[at..].starts_with(p.as_bytes())).copied().unwrap_or(&self.src[at..at + 1]);
        self.i += p.len();
        match p {
            "(" | "[" | "{" => {
                let [before, word] = self.words;
                let head = (matches!(word, "if" | "while" | "for" | "with") && !matches!(before, "." | "?.")) || (word == "await" && before == "for");
                self.deeper(at)?;
                self.open(p.as_bytes()[0], p == "(" && head);
                self.said(p, Last::Other);
            }
            ")" | "]" | "}" => {
                // The `}` that ends a template's `${…}` goes back to its text.
                let closed = self.close();
                // A `}` that closed a `{` ends a block or a value as the parser takes it: a `/`
                // after it reads two ways.
                self.two_ways = p == "}" && closed.as_ref().is_some_and(|l| l.open == b'{');
                let head = closed.is_some_and(|l| l.head);
                self.said(p, if head { Last::Other } else { Last::Value });
            }
            "," => {
                if self.top().angles == 0 {
                    self.sibling();
                }
                self.said(p, Last::Other);
            }
            ";" => {
                self.sibling();
                self.said(p, Last::Other);
            }
            "<" => {
                self.top().angles += 1;
                self.deeper(at)?;
                self.said(p, Last::Other);
            }
            ">" | ">>" | ">>>" => {
                self.deeper(at)?;
                // A `>` closing a type's arguments ends a value: `Array<T>`; or it compares, and
                // a `/` after it starts a regular expression: it reads two ways.
                let level = self.top();
                let closes = level.angles > 0;
                level.angles = level.angles.saturating_sub(p.len());
                self.two_ways = closes;
                self.said(p, if closes { Last::Value } else { Last::Other });
            }
            "++" | "--" => {
                self.deeper(at)?;
                // First on its line it is the next statement's, before a value (`a⏎++b`).
                let last = if line_start { Last::Other } else { self.last };
                self.said(p, last);
            }
            "!" if self.last == Last::Value && !line_start => {
                // After a value on its line, TypeScript's non-null `a!`: a value still.
                self.deeper(at)?;
                self.said(p, Last::Value);
            }
            _ => {
                self.deeper(at)?;
                self.said(p, Last::Other);
            }
        }
        Ok(())
    }
}

fn is_word_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || matches!(c, b'_' | b'$' | b'#' | b'\\') || c >= 0x80
}

fn is_word_part(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'$' | b'\\') || c >= 0x80
}

#[cfg(test)]
mod tests {
    use super::super::{format, ts};
    use super::*;
    use crate::editing;

    /// A flow whose handler runs `body`, with `a` and a function `f` to use in it.
    fn handler(body: &str) -> String {
        format!("import {{ flow, route }} from \"holon\"\n\nflow(\"deep\", {{ port: 8999 }})\n\nfunction f(x) {{\n  return x + 1\n}}\n\nroute.post(\"/\", (req) => {{\n  const a = req.json.a\n{body}}})\n")
    }

    fn answering(e: &str) -> String {
        handler(&format!("  return {e}\n"))
    }

    fn lines(n: usize, line: impl Fn(usize) -> String) -> String {
        (1..=n).map(line).collect()
    }

    /// A flow nesting one way `n` levels deep.
    type Form = fn(usize) -> String;

    /// Each way a text nests, written `n` levels deep. The last three are statements one after
    /// another, which the count takes as siblings, refused by the reader: each return before a
    /// statement not taken holds over it, as does each value a try block makes, and each write
    /// under a condition wraps the one before.
    const FORMS: &[(&str, Form)] = &[
        ("parentheses", |n| answering(&format!("{}a{}", "(".repeat(n), ")".repeat(n)))),
        ("lists", |n| answering(&format!("{}a{}", "[".repeat(n), "]".repeat(n)))),
        ("records", |n| answering(&format!("{}a{}", "{ a: ".repeat(n), " }".repeat(n)))),
        ("calls", |n| answering(&format!("{}a{}", "f(".repeat(n), ")".repeat(n)))),
        ("arrows", |n| answering(&format!("{}a", "x => ".repeat(n)))),
        ("conditionals", |n| answering(&format!("{}0", "a ? 1 : ".repeat(n)))),
        ("nots", |n| answering(&format!("{}a", "!".repeat(n)))),
        ("negations", |n| answering(&format!("{}a", "- ".repeat(n)))),
        ("powers", |n| answering(&format!("a{}", " ** a".repeat(n)))),
        ("sums", |n| answering(&format!("a{}", " + a".repeat(n)))),
        ("fields", |n| answering(&format!("a{}", ".b".repeat(n)))),
        ("templates", |n| answering(&format!("{}a{}", "`${".repeat(n), "}`".repeat(n)))),
        ("types", |n| answering("a").replacen("route.post", &format!("type T = {}number{}\n\nroute.post", "Array<".repeat(n), ">".repeat(n)), 1)),
        ("ifs", |n| handler(&format!("{}return 1\n{}  return 0\n", "if (a) {\n".repeat(n), "}\n".repeat(n)))),
        ("returns", |n| handler(&format!("{}  return 0\n", lines(n, |k| format!("  if (a === {k}) {{\n    return {k}\n  }}\n"))))),
        ("tries", |n| handler(&format!("  try {{\n    const x0 = a\n{}    return x{n}\n  }} catch {{\n    return 0\n  }}\n", lines(n, |k| format!("    const x{k} = f(x{})\n", k - 1))))),
        ("writes", |n| {
            let body = format!("{}  return 0\n", lines(n, |k| format!("  if (a === {k}) {{\n    state.x = {k}\n  }}\n")));
            handler(&body).replacen("\n\nflow(", "\nimport state from \"state\"\n\nflow(", 1)
        }),
    ];

    /// Unicode's spaces, as the parser takes them, between a line's end and what goes on it.
    const SPACES: &[&str] = &["\u{a0}", "\u{feff}", "\u{3000}", "\u{2003}", "\u{200b}", "\u{85}", "\u{1680}", "\u{202f}", "\u{205f}", "\u{b}", "\u{c}"];

    /// `n` parentheses round `a`.
    fn parens(n: usize) -> String {
        format!("{}a{}", "(".repeat(n), ")".repeat(n))
    }

    /// Each way a text was written that the count read otherwise than the parser (audit R36),
    /// `n` deep as the parser reads it: a word that wants what follows it at a line's end, its
    /// statement reset by the next line (`x as⏎any as⏎any…`); an `else if` chain whose statements
    /// end with `;`; a comment, a string or a hashbang the count read past where the lexer ends
    /// it; Unicode's spaces read as words, each beginning a statement; and a `/` the count read as
    /// a division where the parser starts a regular expression, or the other way, skipping what
    /// the other counts (after a TypeScript `!`, a `++` begun on a line, a `}`, an `of`, a `>`).
    const READ_ALIKE: &[(&str, Form)] = &[
        ("`as` at a line's end", |n| answering(&format!("a{}", " as\nany".repeat(n)))),
        ("`satisfies` at a line's end", |n| answering(&format!("a{}", " satisfies\nany".repeat(n)))),
        ("`keyof` at a line's end", |n| answering("a").replacen("route.post", &format!("type T = {}string\n\nroute.post", "keyof\n".repeat(n)), 1)),
        ("`else if` after statements", |n| handler(&format!("  let x = 0\n{}  return x\n", lines(n, |k| format!("  {}if (a === {k}) x = {k};\n", if k == 1 { "" } else { "else " }))))),
        ("a `//` comment a lone `\\r` ends", |n| answering(&format!("// c\r{}", parens(n)))),
        ("a `//` comment U+2028 ends", |n| answering(&format!("// c\u{2028}{}", parens(n)))),
        ("a `//` comment U+2029 ends", |n| answering(&format!("// c\u{2029}{}", parens(n)))),
        ("a hashbang", |n| format!("#!/usr/bin/env holon /*\n{}", answering(&parens(n)))),
        ("a string a `\\` goes on over `\\r\\n`", |n| answering(&format!("\"a\\\r\n\" + {}", parens(n)))),
        ("Unicode's spaces", |n| answering(&format!("a{}", lines(n, |k| format!("\n{}+ a", SPACES[k % SPACES.len()]))))),
        ("a division after TypeScript's `!`", |n| answering(&format!("a! / {} / 1", parens(n)))),
        ("a regular expression after `++` begun on a line", |n| handler(&format!("  let r = {{ lastIndex: 0 }}\n  r\n  ++/'/.lastIndex + {}\n  return 0\n", parens(n)))),
        ("a regular expression after `}`", |n| handler(&format!("  if (a) {{}}\n  /`/\n  return {}\n", parens(n)))),
        ("a division after `of`", |n| handler(&format!("  const of = 1\n  return of / {} / 1\n", parens(n)))),
        ("a regular expression after a type's `>`", |n| handler(&format!("  const r = <any>/`/\n  return {}\n", parens(n)))),
    ];

    /// How deep the parser's tree of `text` goes, in its nodes: oxc's own reading, on a stack
    /// with room for what these tests write.
    fn parser_depth(text: &str) -> usize {
        struct Depth {
            now: usize,
            most: usize,
        }
        impl<'a> oxc_ast_visit::Visit<'a> for Depth {
            fn enter_node(&mut self, _: oxc_ast::AstKind<'a>) {
                self.now += 1;
                self.most = self.most.max(self.now);
            }
            fn leave_node(&mut self, _: oxc_ast::AstKind<'a>) {
                self.now -= 1;
            }
        }
        let text = text.to_string();
        let reading = std::thread::Builder::new().stack_size(512 << 20).spawn(move || {
            let allocator = oxc_allocator::Allocator::default();
            let parsed = oxc_parser::Parser::new(&allocator, &text, oxc_span::SourceType::ts()).parse();
            let mut depth = Depth { now: 0, most: 0 };
            oxc_ast_visit::Visit::visit_program(&mut depth, &parsed.program);
            depth.most
        });
        reading.unwrap().join().unwrap()
    }

    /// A flow reading the secret `k`, sending `value`, made of it, to a host, with `a`.
    fn sending(defs: &str, value: &str) -> String {
        format!("import {{ flow, route }} from \"holon\"\nimport k from \"secret:k\"\nimport api from \"host:api.example.com\"\n\nflow(\"deep\", {{ port: 8999 }})\n\n{defs}\nroute.get(\"/\", async (req) => {{\n  const a = req.query.a\n  const r = await api.get(\"/a\", {{ headers: {{ a, k: {value} }} }})\n  return r.status\n}})\n")
    }

    /// Each way a text made the work of an entry that reads it double with each level it nests
    /// (audit R17), `n` levels deep: a function using the one before twice, with a secret through
    /// them (the walks that follow a secret, at check, deploy and each run, followed each use);
    /// a sum of the sum before, as a collection's key (`numeric`, asking again at each use); and
    /// for fmt's layout (Oxfmt's, laying a call's arguments out again for each way it tries), calls
    /// in calls, templates of calls, and a list method's function calling the next.
    const DOUBLING: &[(&str, Form)] = &[
        ("functions used twice", |n| {
            let defs = lines(n, |k| format!("function f{k}(x: string): string {{\n  return f{}(x) + f{}(x)\n}}\n\n", k - 1, k - 1));
            sending(&format!("function f0(x: string): string {{\n  return x.trim()\n}}\n\n{defs}"), &format!("f{n}(k)"))
        }),
        // And two hundred secrets through them, each followed by a walk of its own, which share
        // what one question may visit.
        ("many secrets through functions used twice", |n| {
            let defs = lines(n, |k| format!("function f{k}(x: string): string {{\n  return f{}(x) + f{}(x)\n}}\n\n", k - 1, k - 1));
            let secrets = lines(SECRETS, |i| format!("import k{i} from \"secret:k{i}\"\n"));
            let all = (1..=SECRETS).map(|i| format!("k{i}")).collect::<Vec<_>>().join(", ");
            let text = sending(&format!("function f0(x: string): string {{\n  return x.trim()\n}}\n\n{defs}"), &format!("f{n}([k, {all}].join(\"\"))"));
            text.replacen("import api", &format!("{secrets}import api"), 1)
        }),
        ("sums of sums", |n| {
            let sums = lines(n, |k| format!("  const a{k} = a{} + a{}\n", k - 1, k - 1));
            handler(&format!("  const a0 = 1\n{sums}  return notes[a{n}]\n")).replacen("\n\nflow(", "\nimport notes from \"collection:notes\"\n\nflow(", 1)
        }),
        ("calls", |n| answering(&format!("{}a{}", "f(".repeat(n), ")".repeat(n)))),
        ("templates of calls", |n| answering(&format!("{}a{}", "`${f(".repeat(n), ")}`".repeat(n)))),
        ("functions written in place", |n| answering(&format!("{}a{}", "[a].map((x) => String(".repeat(n), "))".repeat(n)))),
    ];

    /// How many secrets the form with many reads, each followed by a walk of its own.
    const SECRETS: usize = 100;

    /// How long every entry together may take over one text of [`DOUBLING`], in a debug build:
    /// what was doubling took past it twenty levels in.
    const IN_TIME: std::time::Duration = std::time::Duration::from_secs(20);

    /// Whether `message` is the bound's: the count's, or the reader's for what follows one another.
    fn the_bound(message: &str) -> bool {
        message.contains(&format!("more than {MAX_NESTING} ")) || message.contains("levels deep")
    }

    /// What each entry that compiles a text says of it, by the entry's name: its error, a report's
    /// lines, the editor's mistakes; nothing for a text it takes. `lang::compile` is what the
    /// editor's wasm (`compile_holon`) calls; fmt compiles a text before it lays it out, where it
    /// does (`lay_out`).
    fn compiled(text: &str, lay_out: bool) -> Vec<(&'static str, Vec<String>)> {
        let mut said = vec![
            ("parse", ts::parse(text).err().map(|e| e.to_string()).into_iter().collect()),
            ("compile", crate::lang::compile(text).err().map(|e| e.to_string()).into_iter().collect()),
            ("check", crate::commands::check("deep.holon.ts", text).err),
            ("mistakes", editing::mistakes(text).into_iter().map(|m| m.message).collect()),
            ("graph", crate::graph::graph(text, false).err().into_iter().collect()),
        ];
        if lay_out {
            said.push(("format", format::format(text).err().map(|e| e.to_string()).into_iter().collect()));
        }
        said
    }

    /// What the editor reads a text for as it is typed, which answers whatever the text: a hover
    /// and a completion on the flow's name, on a binding and halfway in (inside the nesting, for
    /// a bracket), and its words' colours.
    fn read_in_the_editor(text: &str) {
        let at = |needle: &str| pos(text.as_bytes(), text.find(needle).expect("in the text"));
        let halfway = pos(text.as_bytes(), text.len() / 2);
        for here in [at("flow("), at("a = req"), halfway] {
            let _ = editing::hover(text, here);
            let _ = editing::complete(text, here);
        }
        let _ = editing::highlight(text);
    }

    /// Each entry takes `form` as deep as the bound lets it, compiling it or saying an ordinary
    /// mistake, and says the bound one level past it and far past it.
    fn every_entry_bounds(name: &str, form: Form) {
        let past_the_bound = |n: usize| ts::parse(&form(n)).err().is_some_and(|e| the_bound(&e.message));
        // The deepest the form is taken, between 1 and well past the reader's bound.
        let (mut taken, mut past) = (1, 8 * MAX_NESTING);
        assert!(!past_the_bound(taken) && past_the_bound(past), "{name}: refused 1 deep, or taken {past} deep");
        while past - taken > 1 {
            let mid = (taken + past) / 2;
            *(if past_the_bound(mid) { &mut past } else { &mut taken }) = mid;
        }
        assert!(taken >= MAX_NESTING / 4, "{name}: refused {past} deep, far short of the bound");
        let text = form(taken);
        // fmt may say that laid out, with the parentheses it adds, the text passes the bound, at
        // the place the text nests deepest.
        let deepest = deepest_at(&text);
        let laid_out = format!("{}:{}: laid out as fmt writes it", deepest.line, deepest.col);
        for (entry, said) in compiled(&text, true) {
            let refuses = said.iter().any(|m| the_bound(m) && !m.starts_with(&laid_out));
            assert!(!refuses, "{name}, {taken} deep: {entry} refuses it: {said:?}");
        }
        read_in_the_editor(&text);
        for n in [past, 40 * MAX_NESTING] {
            let text = form(n);
            for (entry, said) in compiled(&text, true) {
                assert!(said.iter().any(|m| the_bound(m)), "{name}, {n} deep: {entry} doesn't say the bound: {said:?}");
            }
            read_in_the_editor(&text);
        }
    }

    /// No text crashes a program that compiles it, however deep it nests: each entry that reads
    /// a flow's text (`holon check` and the rest of the command line, the language server's
    /// mistakes, hover and completion, the MCP `check`, fmt, the editor's wasm) takes each form
    /// nesting takes as deep as the bound lets it, compiling it or saying an ordinary mistake, and
    /// says the bound's message one level past it and far past it. Each form runs on 1 MiB of
    /// stack, what a Windows program's main thread has (a test's has 2). Needed because the
    /// parser and each walk after it go a call deeper per level and nothing else stops them: a
    /// thousand parentheses overflowed `holon check`'s stack, and a request to the language
    /// server or an agent's `check` could end its process. If it fails, a form gets past the count
    /// (count it in `deepest`) or the reader (bound what it gathers, as `hold` does), or the bound
    /// no longer fits an entry's stack; an overflow ends this test's process without a word.
    /// (`else if` chains are left out: they count, but read slowly long before the bound; and fmt
    /// fmt lays out the forms that nest calls or templates far past its own bound
    /// ([`format::MAX_LAYOUT_NESTING`]) no further: it says so, an ordinary mistake.)
    #[test]
    fn no_text_nested_past_the_bound_crashes_any_compile_entry() {
        std::thread::scope(|s| {
            let forms: Vec<_> = FORMS
                .iter()
                .map(|&(name, form)| {
                    let on_a_main_thread = std::thread::Builder::new().name(format!("{name} on a main thread's stack")).stack_size(1 << 20);
                    on_a_main_thread.spawn_scoped(s, move || every_entry_bounds(name, form)).unwrap()
                })
                .collect();
            for form in forms {
                form.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            }
        });
    }

    /// **Guards:** the count reads a text as the parser does, whatever way it is written: each
    /// way that was read otherwise ([`READ_ALIKE`]), written four times the bound deep as the
    /// parser reads it (its own tree's depth, the oracle), is refused by every entry that compiles
    /// a flow, on 1 MiB of stack, a Windows program's main thread's; none goes on to read it.
    /// **Why:** the count is the only thing between a text and the parser's recursion. Where it
    /// read a text otherwise than the lexer (a comment, a string or a hashbang ended elsewhere, a
    /// space taken for a word, a `/` taken the other way), it skipped what the parser read, or
    /// began a statement where the parser went on one: `x as⏎any as⏎any…` and long `else if`
    /// chains crashed `holon check` at 400,000 links, and an agent's proposal 3,000 deep could
    /// trap the reviewer's tab, whose wasm has a 1 MiB stack (audit R36).
    /// **If it fails:** the row named gets past the count: read it as oxc's lexer does
    /// (`space_at`, `line_ends_at`, `string`, `regex`), keep its statement going where the parser
    /// does (`hangs`, `elses`), or refuse a `/` the parser may read either way (`two_ways`). If
    /// the parser reads a row shallow, the row no longer tests the count: write it again.
    #[test]
    fn no_way_of_writing_a_text_gets_it_past_the_count() {
        let n = 4 * MAX_NESTING;
        std::thread::scope(|s| {
            let rows: Vec<_> = READ_ALIKE
                .iter()
                .map(|&(name, form)| {
                    let on_a_main_thread = std::thread::Builder::new().name(format!("{name} on a main thread's stack")).stack_size(1 << 20);
                    let text = form(n);
                    let read = parser_depth(&text);
                    assert!(read >= n, "{name}: the parser reads it {read} deep, not {n}: the row doesn't test the count");
                    // The count first: a text it lets through would only be read, slowly, by
                    // every entry below.
                    assert!(super::check(&text).is_err(), "{name}: the count lets it through, {read} deep by the parser's tree");
                    on_a_main_thread
                        .spawn_scoped(s, move || {
                            for (entry, said) in compiled(&text, true) {
                                let refused = said.iter().any(|m| m.contains(&format!("more than {MAX_NESTING} levels deep")) || m.contains("reads two ways"));
                                assert!(refused, "{name}: {entry} doesn't refuse it at the count: {said:?}");
                            }
                        })
                        .unwrap()
                })
                .collect();
            for row in rows {
                row.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            }
        });
    }

    /// The samples, the flows Holon is shown by, nest half the bound at most: it sits far above
    /// any flow written to be read. If it fails, a sample nests deeper than it needs (name a part
    /// of it), or flows need a higher bound, which every entry's stack must still fit (the test
    /// above).
    /// Guards: no entry that reads a flow's text (the command line's check and fmt, the language
    /// server's mistakes, hover and completion, an agent's check, the editor's wasm) does work
    /// that doubles with each level the text nests: each form that did, as deep as the reader
    /// takes it up to 60 levels, is read by every entry in [`IN_TIME`] together, and none says the
    /// nesting bound. fmt lays the layout's forms out or says it can't in time.
    /// Why: a function using one that uses one… twice each was followed at every path, so the
    /// walk that finds where a secret goes, at `POST /deploy`, the preview, `holon check` and each
    /// run, was 2^k: eighteen levels took 2.8 s, twenty-four minutes; `a1 = a0 + a0`, `a2 = a1 +
    /// a1`… as a collection's key asked each sum again at each use; and Oxfmt's layout took 2.5 s
    /// for `xs.map((x) => String(…))` six deep and past half a minute at seven (audit R17).
    /// If it fails: an entry walks a text again for each way it reaches a part of it (ask each
    /// part once, or bound the walk, as `taint::MAX_VISITS` does, saying no less than the flow
    /// does), or fmt's layout takes a form past its bound in time (count it in
    /// `format::layout_nesting`).
    #[test]
    fn no_entry_doubles_its_work_with_each_level_a_text_nests() {
        for &(name, form) in DOUBLING {
            let past_the_bound = |n: usize| ts::parse(&form(n)).err().is_some_and(|e| the_bound(&e.message));
            let n = (1..=60).rev().find(|&n| !past_the_bound(n)).expect("taken one level deep");
            assert!(n >= 20, "{name}: refused {} deep", n + 1);
            let text = form(n);
            let started = std::time::Instant::now();
            let (said, read) = std::sync::mpsc::channel();
            let reading = text.clone();
            std::thread::Builder::new()
                .name(format!("{name}, {n} deep"))
                .stack_size(4 << 20)
                .spawn(move || {
                    let entries = compiled(&reading, true);
                    read_in_the_editor(&reading);
                    let _ = said.send(entries);
                })
                .unwrap();
            let entries = match read.recv_timeout(IN_TIME) {
                Ok(entries) => entries,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!("{name}, {n} deep: the entries took more than {IN_TIME:?}"),
                Err(e) => panic!("{name}, {n} deep: an entry failed ({e})"),
            };
            println!("{name}, {n} deep: {:?}", started.elapsed());
            for (entry, said) in &entries {
                assert!(!said.iter().any(|m| the_bound(m)), "{name}, {n} deep: {entry} says the nesting bound: {said:?}");
                if *entry == "format" {
                    assert!(said.iter().all(|m| m.contains("fmt can't lay this out in time")), "{name}, {n} deep: fmt says {said:?}");
                }
            }
            if name.ends_with("functions used twice") {
                // Past what the walks may visit, the secret is taken as going wherever the flow
                // could send it: the one host it calls, which it does send it to.
                let check = entries.iter().find(|(e, _)| *e == "check").map(|(_, s)| s.clone());
                assert_eq!(check, Some(vec![]), "{name}: check refuses it");
                let sent = crate::commands::sent_of(&text);
                assert!(sent.get("k").is_some_and(|to| to.contains("api.example.com")), "{name}: sent {sent:?}");
            }
        }
    }

    /// Guards: the samples lay out at half fmt's bound at most ([`format::MAX_LAYOUT_NESTING`]).
    /// Why: past it fmt refuses a text, so it must sit far above any flow written to be read.
    /// If it fails: a sample nests its calls, templates and functions written in place deeper
    /// than it needs (name a part of it), or the bound is too low for flows people write, and
    /// raising it must keep `no_entry_doubles_its_work_with_each_level_a_text_nests` in time.
    #[test]
    fn the_samples_lay_out_far_below_fmts_bound() {
        let mut most = (0, "");
        for s in crate::samples::samples() {
            let (nests, past) = format::layout_nesting(s.text);
            assert!(past.is_none() && nests <= format::MAX_LAYOUT_NESTING / 2, "{} nests {nests} as fmt's layout counts it", s.name);
            most = most.max((nests, s.name));
        }
        println!("the samples' most, as fmt's layout counts: {} ({})", most.0, most.1);
    }

    #[test]
    fn the_samples_nest_far_below_the_bound() {
        for s in crate::samples::samples() {
            let deepest = deepest(s.text, usize::MAX).expect("no bound");
            assert!(deepest <= MAX_NESTING / 2, "{} nests {deepest} levels deep", s.name);
        }
    }
}
