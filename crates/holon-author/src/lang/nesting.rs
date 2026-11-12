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

/// `src` refused at the place it nests past [`MAX_NESTING`], if it does.
pub fn check(src: &str) -> Result<(), LowerError> {
    match deepest(src, MAX_NESTING) {
        Ok(_) => Ok(()),
        Err(at) => {
            let at = pos(src.as_bytes(), at);
            Err(LowerError {
                span: Span { start: at, end: at },
                message: format!("this nests more than {MAX_NESTING} levels deep: name a part of it, with a const or a function"),
            })
        }
    }
}

/// Where `src` nests deepest, the first place it does.
pub(crate) fn deepest_at(src: &str) -> Pos {
    let most = deepest(src, usize::MAX).unwrap_or_default();
    pos(src.as_bytes(), deepest(src, most.saturating_sub(1)).err().unwrap_or_default())
}

/// The line and column (in characters) of byte `at`.
fn pos(b: &[u8], at: usize) -> Pos {
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

/// The words that put what follows them under them: `typeof x`, `new X`, `a in b`, `do …`.
const NESTING_WORDS: &[&str] = &["typeof", "void", "delete", "await", "yield", "new", "do", "in", "instanceof", "of", "as", "satisfies", "keyof", "infer", "extends", "unique", "asserts", "is", "readonly"];

/// The words after which a value is still to come, so a `/` starts a regular expression.
const BEFORE_A_VALUE: &[&str] = &["return", "typeof", "case", "in", "of", "delete", "void", "throw", "new", "else", "do", "await", "yield", "instanceof", "extends"];

/// The words that go on a statement begun on a line before: `a\nin b`, `} else {`, `} catch {`.
const GOING_ON: &[&str] = &["in", "instanceof", "as", "satisfies", "of", "extends", "implements", "is", "else", "catch", "finally", "while"];

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
    /// The sum of the levels' counts: how deep the token being read is.
    depth: usize,
    /// The deepest so far, and how deep it may go.
    deepest: usize,
    limit: usize,
    last: Last,
    /// The last two tokens, for what a `(` heads.
    words: [&'s str; 2],
    /// Whether a line ended since the last token.
    newline: bool,
}

/// How deep `src` nests, or the byte where it first nests past `limit`.
pub(crate) fn deepest(src: &str, limit: usize) -> Result<usize, usize> {
    let root = Level { open: 0, count: 0, angles: 0, head: false };
    let mut c = Count { src, b: src.as_bytes(), i: 0, levels: vec![root], depth: 0, deepest: 0, limit, last: Last::Other, words: ["", ""], newline: false };
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

    /// One level deeper, in the level the token at `at` is in.
    fn deeper(&mut self, at: usize) -> Result<(), usize> {
        self.top().count += 1;
        self.depth += 1;
        self.deepest = self.deepest.max(self.depth);
        if self.depth > self.limit { Err(at) } else { Ok(()) }
    }

    /// What comes next is a sibling of what came before it in this level.
    fn sibling(&mut self) {
        let level = self.top();
        let count = std::mem::take(&mut level.count);
        self.depth -= count;
    }

    fn open(&mut self, open: u8, head: bool) {
        self.levels.push(Level { open, count: 0, angles: 0, head });
    }

    /// The level closed: the one under it is the token's.
    fn close(&mut self) -> Option<Level> {
        if self.levels.len() == 1 {
            return None;
        }
        let level = self.levels.pop()?;
        self.depth -= level.count;
        Some(level)
    }

    fn said(&mut self, token: &'s str, last: Last) {
        self.words = [self.words[1], token];
        self.last = last;
    }

    /// A template's text, to its end or its next `${`.
    fn template_text(&mut self) -> Result<(), usize> {
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

    /// Code: whitespace and comments, or one token.
    fn code(&mut self) -> Result<(), usize> {
        let c = self.b[self.i];
        match c {
            b'\n' => {
                self.newline = true;
                self.i += 1;
            }
            _ if c.is_ascii_whitespace() => self.i += 1,
            b'/' if self.peek(1) == Some(b'/') => {
                while self.peek(0).is_some_and(|c| c != b'\n') {
                    self.i += 1;
                }
            }
            b'/' if self.peek(1) == Some(b'*') => {
                self.i += 2;
                while let Some(c) = self.peek(0) {
                    if c == b'*' && self.peek(1) == Some(b'/') {
                        self.i += 2;
                        break;
                    }
                    // A comment over lines ends a line, as one does for ending a statement.
                    self.newline |= c == b'\n';
                    self.i += 1;
                }
            }
            _ => {
                if self.newline && self.last == Last::Value && self.starts_statement() {
                    self.sibling();
                }
                self.newline = false;
                self.token()?;
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

    fn word_here(&self) -> &'s str {
        let end = self.b[self.i..].iter().position(|&c| !is_word_part(c)).map_or(self.b.len(), |n| self.i + n);
        &self.src[self.i..end]
    }

    fn token(&mut self) -> Result<(), usize> {
        let at = self.i;
        let c = self.b[at];
        if is_word_start(c) {
            let word = self.word_here();
            self.i += word.len();
            // After a `.`, a word is a property's name, whatever it is.
            let property = matches!(self.words[1], "." | "?.");
            if !property && NESTING_WORDS.contains(&word) {
                self.deeper(at)?;
            }
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
        } else if c == b'/' && self.last != Last::Value {
            self.regex();
            self.said("", Last::Value);
        } else {
            self.punctuation(at)?;
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

    /// A string, to its closing quote or its line's end (where it is a mistake TypeScript stops at).
    fn string(&mut self, quote: u8) {
        self.i += 1;
        while let Some(c) = self.peek(0) {
            match c {
                b'\\' => self.i += 2,
                b'\n' => return,
                _ if c == quote => {
                    self.i += 1;
                    return;
                }
                _ => self.i += 1,
            }
        }
    }

    /// A regular expression, with its flags, to its closing `/` or its line's end.
    fn regex(&mut self) {
        self.i += 1;
        let mut class = false;
        while let Some(c) = self.peek(0) {
            match c {
                b'\\' => self.i += 2,
                b'\n' => return,
                b'/' if !class => {
                    self.i += 1;
                    break;
                }
                _ => {
                    class = (class || c == b'[') && c != b']';
                    self.i += 1;
                }
            }
        }
        while self.peek(0).is_some_and(is_word_part) {
            self.i += 1;
        }
    }

    fn punctuation(&mut self, at: usize) -> Result<(), usize> {
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
                // A `>` closing a type's arguments ends a value: `Array<T>`.
                let level = self.top();
                let closes = level.angles > 0;
                level.angles = level.angles.saturating_sub(p.len());
                self.said(p, if closes { Last::Value } else { Last::Other });
            }
            "++" | "--" => {
                self.deeper(at)?;
                let last = self.last;
                self.said(p, last);
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

    /// The forms fmt's layout (Oxfmt's) takes too long to lay out as deep as the bound, so fmt is
    /// left out for them there: it lays a text out again for each call or template the text is
    /// in, twice the time a level, and these nest a call or a template at every level.
    const SLOW_TO_LAY_OUT: &[&str] = &["calls", "templates"];

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
        for (entry, said) in compiled(&text, !SLOW_TO_LAY_OUT.contains(&name)) {
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
    /// for the forms in [`SLOW_TO_LAY_OUT`] at the bound.)
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

    /// The samples, the flows Holon is shown by, nest half the bound at most: it sits far above
    /// any flow written to be read. If it fails, a sample nests deeper than it needs (name a part
    /// of it), or flows need a higher bound, which every entry's stack must still fit (the test
    /// above).
    #[test]
    fn the_samples_nest_far_below_the_bound() {
        for s in crate::samples::samples() {
            let deepest = deepest(s.text, usize::MAX).expect("no bound");
            assert!(deepest <= MAX_NESTING / 2, "{} nests {deepest} levels deep", s.name);
        }
    }
}
