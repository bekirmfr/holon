//! The heavy tests and the file rules that decide what a change runs (CLAUDE.md, "Heavy tests"):
//! the tests marked heavy are the ones .github/workflows/heavy.yml runs, and every path
//! .github/file-rules.txt names is there.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The start of the mark a heavy test carries, as `#[ignore = "heavy: why"]`.
const MARK: &str = "#[ignore = \"heavy:";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every `.rs` file under `dir`, past what a build writes.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_none_or(|n| n != "target") {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The path from the root, with forward slashes.
fn shown(path: &Path) -> String {
    path.strip_prefix(root()).unwrap_or(path).display().to_string().replace('\\', "/")
}

/// The sections of .github/file-rules.txt, each with its entries in order.
fn file_rules() -> BTreeMap<String, Vec<String>> {
    let text = std::fs::read_to_string(root().join(".github/file-rules.txt")).unwrap();
    let mut rules: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.to_string();
            rules.entry(section.clone()).or_default();
        } else {
            rules.entry(section.clone()).or_default().push(line.to_string());
        }
    }
    rules
}

/// **Guards:** the tests marked heavy (`#[ignore = "heavy: …"]`, which every change's run skips)
/// are exactly the tests .github/workflows/heavy.yml runs, each by its full name; and every path
/// .github/file-rules.txt names is there, in a section what-changed.sh reads, never in a group
/// and its -not section both.
///
/// **Why:** a heavy test is run only where the workflow names it. A test marked heavy and not
/// named there, or renamed, runs nowhere, and nothing says so; a test named there and no longer
/// marked runs in every change's run too, and `--exact` passes a name that matches nothing. A
/// rule naming a path that was moved stops deciding anything, as quietly.
///
/// **If it fails:** it names the test or the path. Mark the test, or name it in the workflow's
/// `GRIDS` or `RECORDS`, or take it out of both; fix the path in file-rules.txt.
#[test]
fn the_heavy_tests_and_the_file_rules_name_what_is_there() {
    let root = root();
    let mut wrong = Vec::new();

    // The tests marked heavy, by their full name in the `holon` binary the workflow runs.
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    let src = root.join("crates/holon/src");
    let mut marked = BTreeSet::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.trim_start().starts_with(MARK) {
                continue;
            }
            let Some(name) = lines[i + 1..].iter().find_map(|l| l.trim_start().strip_prefix("fn ").and_then(|f| f.split('(').next())) else {
                wrong.push(format!("{}:{}: a heavy mark on no function", shown(file), i + 1));
                continue;
            };
            // The workflow runs `cargo test -p holon --bin holon`: a heavy test lives in that
            // binary, at the top of its file's module, so its name follows from its file's path.
            let Ok(rel) = file.strip_prefix(&src) else {
                wrong.push(format!("{}: `{name}` is marked heavy outside crates/holon/src, which the heavy run doesn't build", shown(file)));
                continue;
            };
            if line.starts_with(char::is_whitespace) {
                wrong.push(format!("{}: `{name}` is marked heavy inside a module of its file: put it at the top of a file, as the heavy run names it by its file", shown(file)));
                continue;
            }
            let module = rel.with_extension("").display().to_string().replace('\\', "/");
            let module: Vec<&str> = module.split('/').filter(|p| *p != "mod" && *p != "main").collect();
            marked.insert(module.into_iter().chain([name]).collect::<Vec<_>>().join("::"));
        }
    }
    assert!(marked.len() >= 5, "only {} tests found marked heavy: is the scan reading the sources?", marked.len());

    // The tests the workflow names: every `a::b` path in it.
    let workflow = std::fs::read_to_string(root.join(".github/workflows/heavy.yml")).unwrap();
    let named: BTreeSet<String> = workflow
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
        .filter(|w| w.contains("::") && !w.starts_with(':') && !w.ends_with(':'))
        .map(str::to_string)
        .collect();
    for test in marked.difference(&named) {
        wrong.push(format!("{test} is marked heavy and .github/workflows/heavy.yml doesn't run it"));
    }
    for test in named.difference(&marked) {
        wrong.push(format!(".github/workflows/heavy.yml runs {test}, which no test marked heavy is"));
    }

    // The file rules: every entry there, every section one what-changed.sh reads.
    let rules = file_rules();
    let script = std::fs::read_to_string(root.join(".github/what-changed.sh")).unwrap();
    for (section, entries) in &rules {
        let group = section.strip_suffix("-not").unwrap_or(section);
        if !script.contains(&format!("\"{group}\"")) {
            wrong.push(format!(".github/file-rules.txt has a section [{section}], which what-changed.sh doesn't read"));
        }
        for entry in entries {
            let path = root.join(entry.trim_end_matches('/'));
            let there = if entry.ends_with('/') { path.is_dir() } else { path.is_file() };
            if !there {
                wrong.push(format!(".github/file-rules.txt, [{section}]: {entry} isn't there"));
            }
            if section.ends_with("-not") && rules.get(group).is_some_and(|g| g.contains(entry)) {
                wrong.push(format!(".github/file-rules.txt: {entry} is in [{group}] and [{section}] both"));
            }
        }
    }

    wrong.sort();
    wrong.dedup();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
