//! The docs (the README and docs/manual) stay true to the code: every flow in them (a `ts` block)
//! compiles, as TypeScript, every `holon check` they show is what it prints, and the page of leaves
//! is what the catalog says. The blocks are written to be read, so they needn't be as `holon fmt`
//! writes them.
//!
//! `HOLON_BLESS=1 cargo test -p holon-author --test manual` writes the page of leaves again.

use std::path::{Path, PathBuf};

use holon_author::{agent, commands, samples};

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

/// The pages the tests read, each with the name a failure gives it: the README and the manual's.
fn docs() -> Vec<(String, String)> {
    let read = |path: &Path| std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
    let mut files = vec![];
    pages(&manual(), &mut files);
    let mut out = vec![("README.md".to_string(), read(&manual().join("../../README.md")))];
    for file in &files {
        let name = file.strip_prefix(manual()).unwrap().display().to_string().replace('\\', "/");
        out.push((format!("docs/manual/{name}"), read(file)));
    }
    out
}

/// A fenced block: the line its text starts on, what its fence says it is (`ts`, `ts broken`,
/// `sh`, or nothing), and its text, without the fence's indent.
struct Fence {
    line: usize,
    lang: String,
    body: String,
}

/// Every fenced block of a page, in order.
fn fences(text: &str) -> Vec<Fence> {
    let mut out = vec![];
    let mut open: Option<(usize, Fence)> = None;
    for (i, line) in text.lines().enumerate() {
        let bare = line.trim_start();
        let indent = line.len() - bare.len();
        match &mut open {
            None => {
                if let Some(info) = bare.strip_prefix("```") {
                    open = Some((indent, Fence { line: i + 2, lang: info.trim().to_string(), body: String::new() }));
                }
            }
            Some(_) if bare.trim_end() == "```" => out.push(open.take().unwrap().1),
            Some((fence, block)) => {
                block.body.push_str(&line[indent.min(*fence)..]);
                block.body.push('\n');
            }
        }
    }
    assert!(open.is_none(), "a block isn't closed");
    out
}

#[test]
fn every_flow_in_the_docs_compiles() {
    let mut seen = 0;
    let mut wrong = vec![];
    for (name, text) in docs() {
        for block in fences(&text) {
            let at = format!("{name}:{}.holon.ts", block.line);
            match block.lang.as_str() {
                "holon" => wrong.push(format!("{at}: a flow is written in TypeScript now, in a ts block")),
                "ts" => {
                    seen += 1;
                    let report = commands::check(&at, &block.body);
                    if report.code != 0 {
                        wrong.push(format!("{at}: doesn't compile:\n{}", report.err.join("\n")));
                    }
                }
                "ts broken" if commands::check(&at, &block.body).code == 0 => {
                    wrong.push(format!("{at}: compiles, but its fence says it is broken"))
                }
                _ => {}
            }
        }
    }
    assert!(seen > 0, "no ts blocks found under {}", manual().display());
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
}

/// Whether a line is one only `holon check` prints: its counts, its unit, what the flow may do.
fn printed_by_check(line: &str) -> bool {
    let counts = line.split_once(": ").and_then(|(_, rest)| rest.split_once(", ")).is_some_and(|(nodes, wires)| {
        let count = |s: &str, what: &str| s.split_once(' ').is_some_and(|(n, w)| n.parse::<u64>().is_ok() && w.starts_with(what));
        count(nodes, "node") && count(wires, "wire")
    });
    let unit = line.strip_prefix("unit ").is_some_and(|h| {
        let h = h.trim_end_matches('…');
        !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit())
    });
    counts || unit || line == "it may:" || line.starts_with("it only computes")
}

/// Whether the lines a doc shows are the real ones: a line `…` stands for lines left out, and a
/// line ending in `…` for one that goes on in hex (a hash, shortened).
fn shows(doc: &[&str], real: &[&str]) -> bool {
    match doc.split_first() {
        None => real.is_empty(),
        Some((&"…", rest)) => (0..=real.len()).any(|skip| shows(rest, &real[skip..])),
        Some((line, rest)) => real.split_first().is_some_and(|(r, more)| same_line(line, r) && shows(rest, more)),
    }
}

fn same_line(doc: &str, real: &str) -> bool {
    doc == real
        || doc.strip_suffix('…').and_then(|stem| real.strip_prefix(stem)).is_some_and(|tail| {
            !tail.is_empty() && tail.chars().all(|c| c.is_ascii_hexdigit())
        })
}

/// Guards: every block of output the README or the manual says `holon check` printed (its
/// counts, its routes, its unit, what the flow may do, its mistakes) is what `holon check` prints
/// for that flow today, and names the file it was run on (`$ holon check <file>` first). The flow
/// is the sample of that file's name, or else the nearest `ts` (or `ts broken`) block above.
/// Why: the launch review (B8) found the README's unit and manifest a version behind, and
/// manifests shown under a flow they weren't printed for; a reader takes these blocks as what
/// they'll see, and a manifest shown short of the real one understates what a flow may do.
/// If it fails: run `holon check` on the flow the failure names and put its output in the block
/// (a hash may be shortened to a prefix and `…`; lines left out are a line `…`), or, if the output
/// changed by mistake, fix the code.
#[test]
fn every_holon_check_the_docs_show_is_what_it_prints() {
    let mut seen = 0;
    let mut wrong = vec![];
    for (name, text) in docs() {
        let blocks = fences(&text);
        let mut flow: Option<&Fence> = None;
        for block in &blocks {
            if block.lang == "ts" || block.lang == "ts broken" {
                flow = Some(block);
                continue;
            }
            if !block.lang.is_empty() {
                continue;
            }
            let lines: Vec<&str> = block.body.lines().collect();
            let Some(file) = lines.first().and_then(|l| l.strip_prefix("$ holon check ")) else {
                if lines.iter().any(|l| printed_by_check(l)) {
                    wrong.push(format!("{name}:{}: shows what `holon check` prints, but not the `$ holon check <file>` it printed it for", block.line));
                }
                continue;
            };
            seen += 1;
            let text = match samples::samples().iter().find(|s| format!("{}.holon.ts", s.name) == file) {
                Some(sample) => sample.text.to_string(),
                None => match flow {
                    Some(above) => above.body.clone(),
                    None => {
                        wrong.push(format!("{name}:{}: `{file}` is no sample's file, and no ts block is above it", block.line));
                        continue;
                    }
                },
            };
            let report = commands::check(file, &text);
            let real: Vec<&str> = report.out.iter().chain(&report.err).flat_map(|l| l.lines()).collect();
            if !shows(&lines[1..], &real) {
                wrong.push(format!(
                    "{name}:{}: `holon check {file}` prints\n{}\nbut the block shows\n{}",
                    block.line,
                    real.join("\n"),
                    lines[1..].join("\n")
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
    assert!(seen >= 8, "only {seen} `$ holon check` blocks found in the README and the manual");
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
