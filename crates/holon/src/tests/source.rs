//! Which lines of a Rust file are a test module's, for the tests that sweep the crate's own source
//! (R78): a `#[cfg(test)]` module, from its attribute to the brace that closes it, wherever it sits
//! in the file, the braces counted in code only (strings, chars and comments made [`inert`]). The
//! at-rest writers scan (tests/at_rest.rs, which takes this file by its path), the body-read scan
//! and R50's timing lint (tests.rs) all ask it, so none takes what follows a test module for test
//! code: the writers scan cut each file at its first one, and main.rs declares `mod surfaces;`, a
//! test module, near its top, so nine hundred lines of main.rs went unread. A test module in a
//! file of its own ([`test_files`]) is test code to each of them too, and what it declares.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// `text` (Rust) with its strings, chars and comments made inert for the sweeps of the source: in strings
/// and chars, brackets, operators and commas are spaces (words stay, so `claim["waited_ms"]` still
/// says what it reads); comments are gone. Every line stays where it was.
pub(crate) fn inert(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let quiet = |c: char| if c.is_alphanumeric() || c == '_' || c == '\n' { c } else { ' ' };
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let (c, next) = (chars[i], chars.get(i + 1).copied());
        let after = if i > 0 { chars[i - 1] } else { ' ' };
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                out.push(quiet(chars[i]));
                i += 1;
            }
            i += 2;
        } else if c == 'r' && matches!(next, Some('"' | '#')) && (after == 'b' || !(after.is_alphanumeric() || after == '_')) && chars[i + 1..].iter().find(|&&h| h != '#') == Some(&'"') {
            // A raw string: r"…", r#"…"#.
            let hashes = chars[i + 1..].iter().take_while(|&&h| h == '#').count();
            out.push('"');
            i += 2 + hashes;
            while i < chars.len() && !(chars[i] == '"' && (1..=hashes).all(|k| chars.get(i + k) == Some(&'#'))) {
                out.push(quiet(chars[i]));
                i += 1;
            }
            out.push('"');
            i += 1 + hashes;
        } else if c == '"' {
            out.push('"');
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    out.push(' ');
                    i += 1;
                }
                if i < chars.len() {
                    out.push(quiet(chars[i]));
                    i += 1;
                }
            }
            out.push('"');
            i += 1;
        } else if c == '\'' && (next == Some('\\') || chars.get(i + 2) == Some(&'\'')) {
            // A char ('{', '\'', '\u{1F600}'), not a lifetime.
            let mut end = i + 2;
            if next == Some('\\') {
                end = if chars.get(i + 2) == Some(&'u') { (i..chars.len()).find(|&j| chars[j] == '}').unwrap_or(i) + 1 } else { i + 3 };
            }
            out.extend(std::iter::repeat_n(' ', end + 1 - i));
            i = end + 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// For each line of `code` (made [`inert`]), whether it is a test module's: a `#[cfg(test)]`, the
/// attributes and blank lines after it, and the module it is on, to the line of the brace that
/// closes it (or its `mod name;`, the module being a file of its own).
pub(crate) fn test_modules(code: &str) -> Vec<bool> {
    let lines: Vec<&str> = code.lines().collect();
    let mut test = vec![false; lines.len()];
    let module = |l: &str| ["mod ", "pub mod ", "pub(crate) mod ", "pub(super) mod "].iter().any(|m| l.trim_start().starts_with(m));
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() != "#[cfg(test)]" {
            i += 1;
            continue;
        }
        let mut m = i + 1;
        while m < lines.len() && (lines[m].trim().is_empty() || lines[m].trim_start().starts_with("#[")) {
            m += 1;
        }
        if m == lines.len() || !module(lines[m]) {
            i += 1;
            continue;
        }
        let (mut depth, mut opened, mut j) = (0i32, false, m);
        while j < lines.len() {
            for c in lines[j].chars() {
                match c {
                    '{' => (depth, opened) = (depth + 1, true),
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            if (opened && depth <= 0) || (!opened && lines[j].trim_end().ends_with(';')) {
                break;
            }
            j += 1;
        }
        let last = j.min(lines.len() - 1);
        test[i..=last].fill(true);
        i = last + 1;
    }
    test
}

/// For each line of `text` (Rust, as written), whether it is a test module's ([`test_modules`]).
pub(crate) fn in_test_modules(text: &str) -> Vec<bool> {
    let mut test = test_modules(&inert(text));
    // A last line [`inert`] left blank (a comment) is a line all the same.
    test.resize(text.lines().count(), false);
    test
}

/// Of `files` (Rust sources, read from disk), those a test module is the whole of: declared
/// `#[cfg(test)] mod x;` (x.rs in its declarer's module folder, x/mod.rs, or its `#[path]`), and
/// every module one of those declares in turn. surfaces.rs (main.rs's `mod surfaces;`) is one, and
/// holon-author's golden.rs: never built into what ships.
pub(crate) fn test_files(files: &[PathBuf]) -> BTreeSet<PathBuf> {
    // Each out-of-line module declared at a file's top level: its declarer, its file, and whether
    // a `#[cfg(test)]` is on it.
    let mut declared: Vec<(&PathBuf, PathBuf, bool)> = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        let code = inert(&text);
        let test = test_modules(&code);
        let raw: Vec<&str> = text.lines().collect();
        let folder = match file.file_name().and_then(|n| n.to_str()) {
            Some("mod.rs" | "main.rs" | "lib.rs") => file.parent().map(Path::to_path_buf),
            _ => file.parent().zip(file.file_stem()).map(|(parent, stem)| parent.join(stem)),
        };
        let Some(folder) = folder else { continue };
        for (k, line) in code.lines().enumerate() {
            // One inside an inline module would be under that module's name: none is, here.
            let Some(name) = declared_module(line) else { continue };
            let path = raw[..k.min(raw.len())].iter().rev().map(|l| l.trim()).take_while(|l| l.starts_with("#[")).find_map(|l| l.strip_prefix("#[path = \"")?.strip_suffix("\"]"));
            let child = match path {
                Some(path) => file.parent().unwrap_or(Path::new("")).join(path),
                None if !folder.join(format!("{name}.rs")).exists() && folder.join(name).join("mod.rs").exists() => folder.join(name).join("mod.rs"),
                None => folder.join(format!("{name}.rs")),
            };
            declared.push((file, child, test.get(k).copied().unwrap_or(false)));
        }
    }
    let mut tests = BTreeSet::new();
    loop {
        let before = tests.len();
        for (declarer, child, test) in &declared {
            if *test || tests.contains(*declarer) {
                tests.insert(child.clone());
            }
        }
        if tests.len() == before {
            return tests;
        }
    }
}

/// The name an out-of-line module is declared by at a file's top level, in `line` (made
/// [`inert`]): `mod x;`, `pub(crate) mod x;`.
fn declared_module(line: &str) -> Option<&str> {
    let rest = ["mod ", "pub mod ", "pub(crate) mod ", "pub(super) mod "].iter().find_map(|m| line.trim_end().strip_prefix(m))?;
    let name = rest.strip_suffix(';')?.trim();
    (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')).then_some(name)
}

/// Each function of `code` (made [`inert`]) that has a body, named `Type::name` inside an `impl`
/// (the type's, for a trait's impl too) and `name` outside, with the lines (from 0) from its `fn`
/// to the brace that closes it: what a sweep names a place in the sources by, so a line moved
/// doesn't make it another (R87). A function declared at a line's start alone is seen.
pub(crate) fn fn_spans(code: &str) -> Vec<(String, usize, usize)> {
    let lines: Vec<&str> = code.lines().collect();
    // From line `at`, char `from`: the line of the brace that closes the first block opened
    // there; none if a `;` outside brackets ends the item first (a declaration without a body).
    let close = |at: usize, from: usize| -> Option<usize> {
        let (mut depth, mut opened, mut brackets) = (0i32, false, 0i32);
        for (n, line) in lines.iter().enumerate().skip(at) {
            for c in line.chars().skip(if n == at { from } else { 0 }) {
                match c {
                    '{' => (depth, opened) = (depth + 1, true),
                    '}' => {
                        depth -= 1;
                        if opened && depth == 0 {
                            return Some(n);
                        }
                    }
                    '(' | '[' => brackets += 1,
                    ')' | ']' => brackets -= 1,
                    ';' if !opened && brackets == 0 => return None,
                    _ => {}
                }
            }
        }
        None
    };
    let bare = |line: &str| -> String {
        let mut t = line.trim_start();
        while let Some(rest) = ["pub(crate) ", "pub(super) ", "pub ", "const ", "async ", "unsafe ", "default "].iter().find_map(|q| t.strip_prefix(q)) {
            t = rest;
        }
        t.to_string()
    };
    let ident = |t: &str| t.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<String>();
    let mut impls: Vec<(String, usize, usize)> = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let t = bare(line);
        let Some(rest) = t.strip_prefix("impl").filter(|r| r.starts_with(['<', ' '])) else { continue };
        let head = rest.split('{').next().unwrap_or("");
        // The type: after `for` in a trait's impl, else after the impl's own generics.
        let ty = match head.rsplit_once(" for ") {
            Some((_, ty)) => ty.to_string(),
            None => {
                let (mut depth, mut at) = (0i32, 0);
                for (i, c) in head.char_indices() {
                    match c {
                        '<' => depth += 1,
                        '>' => depth -= 1,
                        _ if depth == 0 && !c.is_whitespace() => {
                            at = i;
                            break;
                        }
                        _ => {}
                    }
                }
                head[at..].to_string()
            }
        };
        let ty = ty.trim().trim_start_matches('&').trim_start_matches("dyn ").trim();
        let last = ty.split('<').next().unwrap_or("").rsplit("::").next().unwrap_or("");
        if let Some(end) = close(n, line.find("impl").unwrap_or(0)) {
            impls.push((ident(last), n, end));
        }
    }
    let mut spans = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let Some(name) = bare(line).strip_prefix("fn ").map(ident).filter(|i| !i.is_empty()) else { continue };
        let Some(end) = close(n, line.find("fn ").unwrap_or(0)) else { continue };
        let owner = impls.iter().filter(|(_, from, to)| *from < n && n <= *to).max_by_key(|(_, from, _)| *from);
        spans.push((owner.map_or(name.clone(), |(ty, _, _)| format!("{ty}::{name}")), n, end));
    }
    spans
}

/// The function of `spans` ([`fn_spans`]) that line `n` (from 0) is in: the innermost.
pub(crate) fn fn_at(spans: &[(String, usize, usize)], n: usize) -> Option<&str> {
    spans.iter().filter(|(_, from, to)| *from <= n && n <= *to).max_by_key(|(_, from, _)| *from).map(|(name, _, _)| name.as_str())
}
