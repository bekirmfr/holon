//! Which lines of a Rust file are a test module's, for the tests that sweep the crate's own source
//! (R78): a `#[cfg(test)]` module, from its attribute to the brace that closes it, wherever it sits
//! in the file, the braces counted in code only (strings, chars and comments made [`inert`]). The
//! at-rest writers scan (tests/at_rest.rs, which takes this file by its path), the body-read scan
//! and R50's timing lint (tests.rs) all ask it, so none takes what follows a test module for test
//! code: the writers scan cut each file at its first one, and main.rs declares `mod surfaces;`, a
//! test module, near its top, so nine hundred lines of main.rs went unread.

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
