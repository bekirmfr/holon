//! The manual (docs/manual) stays true to the code: every `holon` block in it compiles, and the
//! page of leaves is what the catalog says. The blocks are written to be read, so they needn't be
//! as `holon fmt` writes them (it names what it binds its own way, and wraps no line).
//!
//! `HOLON_BLESS=1 cargo test -p holon-author --test manual` writes the page of leaves again.

use std::path::{Path, PathBuf};

use holon_author::{agent, commands};

fn manual() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual")
}

/// Every markdown file under `dir`.
fn pages(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            pages(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// The `holon` blocks of a page: the line each starts on, and its text.
fn holon_blocks(text: &str) -> Vec<(usize, String)> {
    let mut blocks = vec![];
    let mut open: Option<(usize, String)> = None;
    for (i, line) in text.lines().enumerate() {
        match &mut open {
            None if line.trim_end() == "```holon" => open = Some((i + 2, String::new())),
            Some(_) if line.trim_end() == "```" => blocks.push(open.take().unwrap()),
            Some((_, body)) => {
                body.push_str(line);
                body.push('\n');
            }
            None => {}
        }
    }
    assert!(open.is_none(), "a holon block isn't closed");
    blocks
}

#[test]
fn every_flow_in_the_manual_compiles() {
    let mut files = vec![];
    pages(&manual(), &mut files);
    let mut seen = 0;
    let mut wrong = vec![];
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap().replace("\r\n", "\n");
        let name = file.strip_prefix(manual()).unwrap().display().to_string();
        for (line, body) in holon_blocks(&text) {
            seen += 1;
            let at = format!("{name}:{line}");
            let report = commands::check(&at, &body);
            if report.code != 0 {
                wrong.push(format!("{at}: doesn't compile:\n{}", report.err.join("\n")));
            }
        }
    }
    assert!(seen > 0, "no holon blocks found under {}", manual().display());
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
}

#[test]
fn the_page_of_leaves_is_the_catalog() {
    let path = manual().join("language/leaves.md");
    let page = agent::leaves_page();
    if std::env::var_os("HOLON_BLESS").is_some() {
        std::fs::write(&path, &page).unwrap();
        return;
    }
    let kept = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert!(kept == page, "{} is out of date: HOLON_BLESS=1 cargo test -p holon-author --test manual writes it again", path.display());
}
