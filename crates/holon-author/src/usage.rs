//! What an editor's inspector says of a word beside its hover (`holon editor`): the forms it is
//! written in, as the manual's language reference gives them, each with what it means (read from
//! the page itself, so the two can't differ, and the page's flows compile), and lines of a sample
//! that use it (samples compile, so the lines are real).

use crate::agent;
use crate::lang::ast::Pos;
use crate::samples;

/// The manual's language reference: its tables' forms, and its plain code blocks' lines.
const REFERENCE: &str = include_str!("../../../docs/manual/language/reference.md");

/// The forms and an example for a word.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Usage {
    /// The forms it is written in, each with what the reference says it means (empty for a line
    /// of a code block, which says it in its comment).
    pub forms: Vec<(String, String)>,
    /// A sample that uses it: the sample's name and its lines.
    pub example: Option<(&'static str, Vec<String>)>,
}

/// The reference's forms, in its order: each code span in a table row's first cell, with the
/// row's second cell; the form a bullet starts with, in bold, with the rest of its line; and each
/// line of a plain (not `ts`) code block.
fn forms() -> Vec<(String, String)> {
    let mut out = vec![];
    let mut fenced = None;
    for line in REFERENCE.lines() {
        if let Some(lang) = line.strip_prefix("```") {
            fenced = if fenced.is_some() { None } else { Some(lang.trim().to_string()) };
            continue;
        }
        match fenced.as_deref() {
            Some("") if !line.trim().is_empty() => out.push((line.trim_end().to_string(), String::new())),
            Some(_) => {}
            None if line.starts_with("- **`") => {
                let Some((code, rest)) = line[4..].split_once("`**") else { continue };
                out.push((code.trim_matches('`').to_string(), plain(rest.trim())));
            }
            None => {
                let cells: Vec<&str> = cells(line);
                let [first, second, ..] = cells[..] else { continue };
                if first.chars().all(|c| c == '-' || c == ':') {
                    continue;
                }
                for code in spans(first) {
                    out.push((code, plain(second)));
                }
            }
        }
    }
    out
}

/// A table row's cells, `\|` kept inside them; nothing for a line that isn't a row.
fn cells(line: &str) -> Vec<&str> {
    let Some(inner) = line.trim().strip_prefix('|').and_then(|l| l.strip_suffix('|')) else { return vec![] };
    let mut out = vec![];
    let (mut from, mut prev) = (0, ' ');
    for (i, c) in inner.char_indices() {
        if c == '|' && prev != '\\' {
            out.push(inner[from..i].trim());
            from = i + 1;
        }
        prev = c;
    }
    out.push(inner[from..].trim());
    out
}

/// The code spans of a cell, `\|` read as `|`.
fn spans(cell: &str) -> Vec<String> {
    let (mut out, mut rest) = (vec![], cell);
    while let Some(a) = rest.find('`') {
        let ticks = rest[a..].chars().take_while(|c| *c == '`').count();
        let fence = "`".repeat(ticks);
        let Some(b) = rest[a + ticks..].find(&fence) else { break };
        out.push(rest[a + ticks..a + ticks + b].trim().replace("\\|", "|"));
        rest = &rest[a + ticks + b + ticks..];
    }
    out
}

/// A cell as plain text: its links' words, `\|` as `|`.
fn plain(cell: &str) -> String {
    let mut out = String::new();
    let mut rest = cell.replace("\\|", "|");
    while let Some(a) = rest.find("](") {
        let Some(b) = rest[a..].find(')') else { break };
        out.push_str(&rest[..a].replacen('[', "", 1));
        rest = rest[a + b + 1..].to_string();
    }
    out.push_str(&rest);
    out
}

/// Whether `form` has `needle` as a word of its own: `route` in `route.METHOD(…)`, not in
/// `router`; `.list` and `state.` as written.
fn has(form: &str, needle: &str) -> bool {
    at_word(form, needle).is_some()
}

/// The word at `at`, and the name before its dot when it is a member (`state` of `state.n`).
fn word_at(source: &str, at: Pos) -> Option<(String, Option<String>)> {
    let line: Vec<char> = source.lines().nth(at.line as usize - 1)?.chars().collect();
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let i = at.col as usize - 1;
    if !line.get(i).copied().is_some_and(ident) {
        return None;
    }
    let start = (0..=i).rev().take_while(|&k| ident(line[k])).last()?;
    let end = (i..line.len()).take_while(|&k| ident(line[k])).last()? + 1;
    let word: String = line[start..end].iter().collect();
    // Inside a comment or a string, it isn't a name.
    let before: String = line[..start].iter().collect();
    if before.contains("//") || before.chars().filter(|c| matches!(c, '"' | '\'' | '`')).count() % 2 == 1 {
        return None;
    }
    if word.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    let head = (start >= 2 && line[start - 1] == '.').then(|| {
        let h = (0..start - 1).rev().take_while(|&k| ident(line[k])).last()?;
        Some(line[h..start - 1].iter().collect::<String>())
    });
    Some((word, head.flatten()))
}

/// What the text imports `name` as: the source's kind (`collection`, `folder`, `secret`, …), or
/// `holon` for the vocabulary and the leaves.
fn imported(source: &str, name: &str) -> Option<String> {
    for line in source.lines().map(str::trim).filter(|l| l.starts_with("import ")) {
        let (names, from) = line.trim_start_matches("import ").split_once(" from ")?;
        let from = from.trim().trim_end_matches(';').split(" with ").next()?.trim().trim_matches('"');
        let bound = names.trim_matches(|c| c == '{' || c == '}' || c == ' ').split(',').any(|n| n.split(" as ").last().is_some_and(|n| n.trim() == name));
        if bound {
            return Some(from.split(':').next().unwrap_or(from).to_string());
        }
    }
    None
}

/// The name the reference writes an imported store or host by, for each kind.
fn placeholder(kind: &str) -> Option<&'static str> {
    match kind {
        "collection" => Some("coll"),
        "folder" => Some("dir"),
        "database" => Some("db"),
        "host" => Some("api"),
        "secret" => Some("key"),
        "channel" => Some("updates"),
        _ => None,
    }
}

/// What the inspector says of the word at `at`, beside its hover: the forms it is written in,
/// and a sample's lines that use it. Nothing for a word that isn't a name.
pub fn usage(source: &str, at: Pos) -> Option<Usage> {
    let (word, head) = word_at(source, at)?;
    let all = forms();
    let pick = |needle: &str| -> Vec<(String, String)> { all.iter().filter(|(f, _)| has(f, needle)).cloned().collect() };
    let (forms, needle) = if let Some(head) = &head {
        // A member: `state.n`, `api.get`, `notes.list`, `t.toUpperCase`.
        match imported(source, head).as_deref() {
            Some("state") => (pick("state."), "state.".to_string()),
            Some(kind) if placeholder(kind).is_some() => {
                let p = placeholder(kind).unwrap_or_default();
                let named = pick(&format!("{p}.{word}"));
                (if named.is_empty() { pick(&format!("{p}.")) } else { named }, format!(".{word}("))
            }
            _ if head == "route" || head == "on" || head == "file" => (pick(&format!("{head}.")), format!("{head}.")),
            _ => {
                // A method: the leaf that does it.
                let named = pick(&format!(".{word}("));
                (if named.is_empty() { pick(&format!("{head}.{word}")) } else { named }, format!(".{word}("))
            }
        }
    } else if let Some(kind) = imported(source, &word).filter(|k| k != "holon") {
        // A name the text imports: how it is imported, and what it is used as.
        let mut forms: Vec<(String, String)> = all.iter().filter(|(f, _)| f.contains(&format!("\"{kind}"))).cloned().collect();
        if let Some(p) = placeholder(&kind) {
            forms.extend(pick(&format!("{p}[")));
            forms.extend(pick(&format!("{p}.")));
        }
        (forms, format!("\"{kind}:"))
    } else if ["route", "every", "on", "action", "flow", "js", "respond", "status", "event", "stripe"].contains(&word.as_str()) {
        // The vocabulary: each form that writes it.
        let mut forms = pick(&format!("{word}("));
        forms.extend(pick(&format!("{word}.")));
        forms.dedup();
        (forms, if word == "route" || word == "on" || word == "stripe" { format!("{word}.") } else { format!("{word}(") })
    } else if agent::reference(Some(&word)).is_ok_and(|r| !r.starts_with('#')) {
        // A leaf, called by its name.
        (pick("leaf("), format!("{word}("))
    } else {
        // A name the text binds: the binding is its form, and the text its example.
        let bound = source.lines().any(|l| {
            let l = l.trim_start();
            ["const ", "let ", "var "].iter().any(|k| l.strip_prefix(k).is_some_and(|r| r.starts_with(&word) && r[word.len()..].trim_start().starts_with(['=', ':'])))
        });
        if !bound {
            return None;
        }
        return Some(Usage { forms: pick("const name = expr"), example: None });
    };
    // A leaf a sample writes as a method (`.split(…)`) has that line.
    let example = example(&needle).or_else(|| needle.ends_with('(').then(|| example(&format!(".{needle}"))).flatten());
    Some(Usage { forms, example })
}

/// A line without its comment.
fn code_of(line: &str) -> &str {
    let mut quote = None;
    let mut prev = ' ';
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'' | '`') => quote = Some(c),
            (Some(q), _) if c == q && prev != '\\' => quote = None,
            (None, '/') if prev == '/' => return &line[..i - 1],
            _ => {}
        }
        prev = c;
    }
    line
}

/// Where `needle` is in `code` as a word of its own: the word's first place.
fn at_word(code: &str, needle: &str) -> Option<usize> {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(k) = code[from..].find(needle).map(|k| k + from) {
        let before = code[..k].chars().next_back();
        let after = code[k + needle.len()..].chars().next();
        let starts = !needle.starts_with(ident) || before.is_none_or(|c| !ident(c) && c != '.');
        let ends = !needle.ends_with(ident) || after.is_none_or(|c| !ident(c));
        if starts && ends {
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
    for s in samples::samples() {
        let lines: Vec<&str> = s.text.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            let code = code_of(line).trim_end();
            let Some(k) = at_word(code, needle) else { continue };
            let first = k == code.len() - code.trim_start().len();
            let open = code.matches(['(', '[', '{']).count() as isize - code.matches([')', ']', '}']).count() as isize;
            let taken = if open > 0 { block(&lines, n) } else if open == 0 { Some(vec![line.to_string()]) } else { None };
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

/// The statement that opens on line `n`, when it is at most eight lines; else its first line, an
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
    use super::{forms, usage};
    use crate::lang::ast::Pos;

    const BUMP: &str = r#"import { action, flow, split } from "holon"
import state from "state"
import notes from "collection:notes"

flow("bump")

export const bump = action(() => {
  const n = (state.n ?? 0) + 1
  state.n = n
  notes["k"] = split("a,b", ",")
  return n
})
"#;

    fn at(line: u32, col: u32) -> Pos {
        Pos { line, col }
    }

    #[test]
    fn the_forms_are_the_references_own() {
        let forms = forms();
        let has = |code: &str| forms.iter().any(|(f, _)| f == code);
        for code in ["state.key = expr", "coll.list({ prefix: p, after: a, limit: n })", "route.METHOD(\"/path\", (req) => answer, { resumes, log })", "leaf(input, input, { name: value, … })"] {
            assert!(has(code), "{code}: {forms:#?}");
        }
        let (_, meaning) = forms.iter().find(|(f, _)| f == "a ?? b").unwrap();
        assert!(meaning.contains("no value"), "{meaning}");
        assert!(forms.iter().all(|(f, _)| !f.contains("\\|")), "a cell's \\| reads as |");
    }

    #[test]
    fn a_word_says_its_forms_and_a_samples_lines() {
        // The vocabulary: the forms that write it, and a sample's.
        let u = usage(BUMP, at(7, 21)).expect("action");
        assert!(u.forms.iter().any(|(f, _)| f.contains("action((input) =>")), "{:#?}", u.forms);
        assert!(u.example.as_ref().is_some_and(|(_, lines)| lines[0].contains("action(")), "{:?}", u.example);

        // A member of the state: what reads and writes it.
        let u = usage(BUMP, at(9, 9)).expect("state.n");
        assert!(u.forms.iter().any(|(f, m)| f == "state.key = expr" && m.contains("when the run finishes")), "{:#?}", u.forms);
        assert!(u.example.is_some());

        // A leaf: the call's form, and a sample's call.
        let u = usage(BUMP, at(10, 17)).expect("split");
        assert!(u.forms[0].0.starts_with("leaf("), "{:#?}", u.forms);
        assert!(u.example.as_ref().is_some_and(|(_, lines)| lines.iter().any(|l| l.contains("split("))), "{:?}", u.example);

        // A name the text imports, and one it binds.
        let u = usage(BUMP, at(10, 4)).expect("notes");
        assert!(u.forms.iter().any(|(f, _)| f.contains("\"collection:NAME\"")), "{:#?}", u.forms);
        assert!(u.forms.iter().any(|(f, _)| f.starts_with("coll[")), "{:#?}", u.forms);
        let u = usage(BUMP, at(8, 9)).expect("n");
        assert!(u.forms.iter().any(|(f, _)| f == "const name = expr") && u.example.is_none(), "{u:?}");

        // Nothing for what isn't a name.
        assert_eq!(usage(BUMP, at(8, 13)), None);
        assert_eq!(usage(BUMP, at(10, 25)), None, "inside a string");
    }
}
