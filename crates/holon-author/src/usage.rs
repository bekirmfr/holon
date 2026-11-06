//! What an editor's inspector says of a word beside its hover (`holon editor`): the grammar it
//! is written by, as docs/LANGUAGE.md §5 gives it (read from the document itself, so the two
//! can't differ), and lines of a sample that use it (samples compile, so the lines are real). The
//! samples are written in TypeScript's syntax now, which this grammar isn't, so none is shown
//! until the inspector reads that form (docs/L4s/typescript-syntax-L4.md, slice 4).

use crate::agent;
use crate::lang::ast::{HeaderKind, Pos};
use crate::lang::lex::{lex, Tok};
use crate::lang::parse;
use crate::samples;

/// The language's own description: its §5 grammar is the one shown.
const LANGUAGE: &str = include_str!("../../../docs/LANGUAGE.md");

/// The grammar and an example for a word.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Usage {
    /// The rules it is written by, each a line as §5 writes them (`name = alternative`, the
    /// rule's other alternatives on lines of their own, `| …`).
    pub grammar: Vec<String>,
    /// A sample that uses it: the sample's name and its lines.
    pub example: Option<(&'static str, Vec<String>)>,
}

/// A rule of the grammar: its name and its alternatives, each with its comment.
struct Rule {
    name: String,
    alts: Vec<String>,
}

/// The rules the fenced blocks of §5 write: a line `name = …` starts one, a line `| …` under it
/// adds alternatives.
fn rules() -> Vec<Rule> {
    let section = LANGUAGE.split_once("\n## 5.").map_or("", |(_, s)| s);
    let section = section.split_once("\n## 6.").map_or(section, |(s, _)| s);
    let mut rules: Vec<Rule> = vec![];
    let (mut fenced, mut open) = (false, false);
    for line in section.lines() {
        if line.starts_with("```") {
            (fenced, open) = (!fenced, false);
            continue;
        }
        if !fenced {
            continue;
        }
        if let Some((name, rhs)) = production(line) {
            rules.push(Rule { name, alts: alternatives(rhs) });
            open = true;
        } else if let Some(rest) = line.trim_start().strip_prefix('|').filter(|_| open && line.starts_with(' ')) {
            if let Some(r) = rules.last_mut() {
                r.alts.extend(alternatives(rest));
            }
        } else {
            open = false;
        }
    }
    rules
}

/// `name = rhs`, a rule's first line: a lowercase name at the line's start.
fn production(line: &str) -> Option<(String, &str)> {
    let (name, rhs) = line.split_once('=')?;
    let name = name.trim_end();
    let named = !name.is_empty() && !line.starts_with(' ') && name.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    named.then(|| (name.to_string(), rhs.trim()))
}

/// A rule's text split at its `|`s (not those in quotes or brackets), its comment kept on the
/// last.
fn alternatives(rhs: &str) -> Vec<String> {
    let (mut alts, mut now) = (vec![], String::new());
    let (mut depth, mut quoted) = (0i32, false);
    let mut comment = None;
    for (i, c) in rhs.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '(' | '[' if !quoted => depth += 1,
            ')' | ']' if !quoted => depth -= 1,
            '#' if !quoted => {
                comment = Some(rhs[i + 1..].trim());
                break;
            }
            '|' if !quoted && depth == 0 => {
                alts.push(std::mem::take(&mut now));
                continue;
            }
            _ => {}
        }
        now.push(c);
    }
    alts.push(now);
    let mut alts: Vec<String> = alts.into_iter().map(|a| a.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|a| !a.is_empty()).collect();
    if let (Some(c), Some(last)) = (comment, alts.last_mut()) {
        last.push_str(&format!("   # {c}"));
    }
    alts
}

/// A whole rule, as lines: its first alternative after its name, the others under it.
fn whole(r: &Rule) -> Vec<String> {
    let pad = " ".repeat(r.name.len() + 1);
    r.alts.iter().enumerate().map(|(i, a)| if i == 0 { format!("{} = {a}", r.name) } else { format!("{pad}| {a}") }).collect()
}

/// Rules shown under another, for the words it is made of; not those every rule uses.
const ALONE: &[&str] = &["file", "item", "header", "handler", "statement", "block", "holon", "route", "call", "args", "arg", "value", "port"];

/// The rules a terminal (`route`, `"after"`) is written in: each alternative that has it, a rule
/// named for it whole, then the rules those are made of.
fn written_with(rules: &[Rule], word: &str) -> Vec<String> {
    let quoted = format!("\"{word}\"");
    let mut out = vec![];
    let mut shown: Vec<&str> = vec![];
    if let Some(r) = rules.iter().find(|r| r.name == word) {
        out.extend(whole(r));
        shown.push(&r.name);
    }
    for r in rules.iter().filter(|r| r.name != word) {
        for a in r.alts.iter().filter(|a| a.contains(&quoted)) {
            out.push(format!("{} = {a}", r.name));
        }
    }
    // What those are made of, once each.
    let made_of: Vec<String> = out.iter().flat_map(|l| words(l)).collect();
    for name in made_of {
        if let Some(r) = rules.iter().find(|r| r.name == name && !ALONE.contains(&r.name.as_str()) && !shown.contains(&r.name.as_str())) {
            shown.push(&r.name);
            out.extend(whole(r));
        }
    }
    out
}

/// A rule line's names: the words outside its quotes and its comment, after its own name.
fn words(line: &str) -> Vec<String> {
    let body = line.split_once(" = ").map_or(line, |(_, b)| b);
    let body = body.split("   # ").next().unwrap_or_default();
    let mut out = vec![];
    let (mut quoted, mut now) = (false, String::new());
    for c in body.chars().chain([' ']) {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && (c.is_ascii_lowercase() || c == '_') {
            now.push(c);
            continue;
        }
        if !now.is_empty() {
            out.push(std::mem::take(&mut now));
        }
    }
    out
}

/// Whether `rules` write `word` as a terminal.
fn is_terminal(rules: &[Rule], word: &str) -> bool {
    let quoted = format!("\"{word}\"");
    rules.iter().any(|r| r.alts.iter().any(|a| a.contains(&quoted)))
}

/// The alternatives of `rule` that start with `start`.
fn starting(rules: &[Rule], rule: &str, start: &str) -> Vec<String> {
    rules.iter().filter(|r| r.name == rule).flat_map(|r| r.alts.iter().filter(|a| a.starts_with(start)).map(|a| format!("{} = {a}", r.name))).collect()
}

/// The rules a call is written by.
fn calls(rules: &[Rule]) -> Vec<String> {
    rules.iter().filter(|r| ["call", "args", "arg"].contains(&r.name.as_str())).flat_map(whole).collect()
}

/// What the inspector says of the word at `at`, beside its hover: the grammar it is written by,
/// and a sample's lines that use it. Nothing for a word that isn't a name.
pub fn usage(source: &str, at: Pos) -> Option<Usage> {
    let tokens = lex(source).ok()?;
    let i = tokens.iter().position(|t| t.tok == Tok::Ident && (at.line, at.col) >= (t.span.start.line, t.span.start.col) && (at.line, at.col) < (t.span.end.line, t.span.end.col))?;
    let word = tokens[i].text.as_str();
    let before = |n: usize| i.checked_sub(n).map(|j| &tokens[j]);
    let member = before(1).is_some_and(|t| t.tok == Tok::Dot).then(|| before(2).filter(|t| t.tok == Tok::Ident).map(|t| t.text.as_str())).flatten();
    let file = parse(source).ok();
    let declared = file.iter().flat_map(|f| &f.headers).find_map(|h| match &h.kind {
        HeaderKind::Collection(c) if c == word => Some("collection"),
        HeaderKind::Folder(f) if f == word => Some("folder"),
        HeaderKind::Channel(c) if c == word => Some("channel"),
        _ => None,
    });
    let rules = rules();
    let (grammar, needle) = if let Some(head) = member {
        // A member: `state.n`, `http.get`, `notes.list`.
        let grammar = if is_terminal(&rules, head) { written_with(&rules, head) } else { calls(&rules) };
        // The same member if a sample has it (`http.get`), else the noun's (`state.`), else the method's.
        let needle = [format!("{head}.{word}"), format!("{head}."), format!(".{word}(")].into_iter().find(|n| word_in_samples(n)).unwrap_or_default();
        (grammar, needle)
    } else if let Some(kind) = declared {
        // A name this text declares: how it is declared, and written to.
        let mut grammar = starting(&rules, "header", &format!("\"{kind}\""));
        grammar.extend(starting(&rules, "statement", "ident \"[\""));
        grammar.extend(starting(&rules, "statement", "\"delete\""));
        (grammar, kind.to_string())
    } else if is_terminal(&rules, word) {
        (written_with(&rules, word), word.to_string())
    } else if agent::reference(Some(word)).is_ok_and(|r| !r.starts_with('#')) {
        // A leaf or a template, called by its name.
        (calls(&rules), format!("{word}("))
    } else {
        // A name the text binds: the binding is its grammar, and the text its example.
        let bound = tokens.iter().zip(tokens.iter().skip(1)).any(|(a, b)| a.tok == Tok::Ident && a.text == word && b.tok == Tok::Eq);
        if !bound {
            return None;
        }
        return Some(Usage { grammar: starting(&rules, "statement", "ident \"=\""), example: None });
    };
    Some(Usage { grammar, example: example(&needle) })
}

/// The samples written in the form this grammar is of.
fn holon_samples() -> impl Iterator<Item = &'static samples::Sample> {
    samples::samples().iter().filter(|s| !crate::lang::is_typescript(s.text))
}

/// Whether some sample has `needle` where its code is.
fn word_in_samples(needle: &str) -> bool {
    holon_samples().any(|s| s.text.lines().any(|l| at_word(code_of(l), needle).is_some()))
}

/// A line without its comment.
fn code_of(line: &str) -> &str {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Where `needle` is in `code` as a word of its own, outside quotes: the word's first place.
fn at_word(code: &str, needle: &str) -> Option<usize> {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(k) = code[from..].find(needle).map(|k| k + from) {
        let before = code[..k].chars().next_back();
        let after = code[k + needle.len()..].chars().next();
        let starts = needle.starts_with('.') || before.is_none_or(|c| !ident(c) && c != '.');
        let ends = !needle.ends_with(ident) || after.is_none_or(|c| !ident(c));
        let quoted = code[..k].matches('"').count() % 2 == 1;
        if starts && ends && !quoted {
            return Some(k);
        }
        from = k + needle.len();
    }
    None
}

/// The best line of the samples that has `needle`: one that starts with it, near forty
/// characters, a complete statement; a block it opens with it, when short.
fn example(needle: &str) -> Option<(&'static str, Vec<String>)> {
    let mut best: Option<(usize, &'static str, Vec<String>)> = None;
    for s in holon_samples() {
        let lines: Vec<&str> = s.text.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            let code = code_of(line).trim_end();
            let Some(k) = at_word(code, needle) else { continue };
            let first = k == code.len() - code.trim_start().len();
            let open = code.matches(['(', '[', '{']).count() as isize - code.matches([')', ']', '}']).count() as isize;
            let taken = if open > 0 && code.ends_with('{') { block(&lines, n) } else if open == 0 { Some(vec![line.to_string()]) } else { None };
            let Some(taken) = taken else { continue };
            let width = taken.iter().map(|l| l.trim().chars().count()).max().unwrap_or(0);
            let score = (!first as usize) * 30 + width.abs_diff(40) + taken.len() * 2;
            if best.as_ref().is_none_or(|(b, ..)| score < *b) {
                best = Some((score, s.name, taken));
            }
        }
    }
    let (_, name, lines) = best?;
    let indent = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    Some((name, lines.iter().map(|l| l.get(indent..).unwrap_or(l.trim()).trim_end().to_string()).collect()))
}

/// The block that opens on line `n`, when it is at most eight lines; else its first line, an
/// ellipsis and its last.
fn block(lines: &[&str], n: usize) -> Option<Vec<String>> {
    let mut depth = 0isize;
    for (m, line) in lines.iter().enumerate().skip(n) {
        let code = code_of(line);
        depth += code.matches(['(', '[', '{']).count() as isize - code.matches([')', ']', '}']).count() as isize;
        if depth <= 0 {
            let all = &lines[n..=m];
            if all.len() <= 8 {
                return Some(all.iter().map(|l| l.to_string()).collect());
            }
            let indent = lines[n].len() - lines[n].trim_start().len();
            return Some(vec![lines[n].to_string(), format!("{}  …", " ".repeat(indent)), lines[m].to_string()]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{rules, usage};
    use crate::lang::ast::Pos;

    const BUMP: &str = "flow bump\ncollection notes\n\naction bump (input) {\n  n = (state.n ?? 0) + 1\n  state.n = n\n  notes[\"k\"] = split(\"a,b\", \",\")\n  respond n\n}\n";

    fn at(line: u32, col: u32) -> Pos {
        Pos { line, col }
    }

    #[test]
    fn the_grammar_is_the_documents_own() {
        let rules = rules();
        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        for name in ["file", "header", "handler", "statement", "route", "call", "respond_args", "tied"] {
            assert!(names.contains(&name), "{names:?}");
        }
        let header = rules.iter().find(|r| r.name == "header").unwrap();
        assert!(header.alts.contains(&"\"port\" number".to_string()), "{:?}", header.alts);
        assert!(rules.iter().all(|r| !r.alts.is_empty() && r.alts.iter().all(|a| !a.contains('|') || a.contains('(') || a.contains('"'))), "split at its |s");
    }

    #[test]
    fn a_word_says_its_grammar_and_a_samples_lines() {
        // A keyword: the alternatives that write it, and what they are made of.
        let u = usage(BUMP, at(8, 3)).expect("respond");
        assert!(u.grammar.iter().any(|g| g.starts_with("statement = \"respond\" respond_args")), "{:#?}", u.grammar);
        assert!(u.grammar.iter().any(|g| g.starts_with("respond_args = ")), "{:#?}", u.grammar);
        // No sample is written in this form now, so no line of one is shown.
        assert_eq!(u.example, None);

        // A block's keyword: the block, when short.
        let u = usage(BUMP, at(4, 1)).expect("action");
        assert!(u.grammar.iter().any(|g| g.contains("\"action\" ident")), "{:#?}", u.grammar);

        // A member of a noun: what writes it.
        let u = usage(BUMP, at(6, 9)).expect("state.n");
        assert!(u.grammar.iter().any(|g| g.contains("\"state\" \".\" ident \"=\" expr")), "{:#?}", u.grammar);

        // A leaf: the call's rules, and a sample's call.
        let u = usage(BUMP, at(7, 17)).expect("split");
        assert!(u.grammar[0].starts_with("call = ident \"(\""), "{:#?}", u.grammar);

        // A name the text declares, and one it binds.
        let u = usage(BUMP, at(7, 4)).expect("notes");
        assert!(u.grammar.iter().any(|g| g.contains("\"collection\" ident")), "{:#?}", u.grammar);
        let u = usage(BUMP, at(5, 3)).expect("n");
        assert!(u.grammar.iter().any(|g| g.starts_with("statement = ident \"=\" expr")) && u.example.is_none(), "{u:?}");

        // Nothing for what isn't a name.
        assert_eq!(usage(BUMP, at(5, 5)), None);
    }
}
