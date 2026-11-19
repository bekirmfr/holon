//! The docs (the README and docs/manual) stay true to the code: every flow in them (a `ts` block)
//! compiles, as TypeScript, every `holon check` they show is what it prints, and the page of leaves
//! is what the catalog says. The blocks are written to be read, so they needn't be as `holon fmt`
//! writes them. What the engineering docs and the code cite (a test by its name, a docs page by
//! its path) is there.
//!
//! `HOLON_BLESS=1 cargo test -p holon-author --test manual` writes the page of leaves again.

use std::collections::{BTreeMap, BTreeSet};
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

/// Every file under `dir` whose name ends with one of `ends`, past what a build or a package
/// manager writes, and past other worktrees.
fn sources(dir: &Path, ends: &[&str], out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if path.is_dir() {
            if !["target", "node_modules", ".git", ".claude", "dist", "wasm"].contains(&name.as_str()) {
                sources(&path, ends, out);
            }
        } else if ends.iter().any(|e| name.ends_with(e)) {
            out.push(path);
        }
    }
}

/// Guards: a test or function a page of the engineering docs, the manual or the README names (a
/// `snake_case` name of four words or more, in backticks) is one the code has, and a docs page the
/// code or those pages name (`docs/….md`) is there.
/// Why: these pages say what the code does by naming the test that holds it. A test renamed or
/// dropped, or a design note never added, leaves a page vouching for what nothing holds, and
/// nobody reading it can tell (release audit R49: security.md cited two tests gone with the old
/// secrets key, and the encryption-at-rest design every module cites wasn't in the repository).
#[test]
fn what_the_docs_and_the_code_cite_is_there() {
    let root = manual().join("../..");
    let read = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
    let shown = |p: &Path| p.strip_prefix(&root).unwrap().display().to_string().replace('\\', "/");
    let mut code = vec![];
    sources(&root, &[".rs", ".ts", ".tsx", ".js", ".mjs", ".py", ".yml", ".toml"], &mut code);
    let words: std::collections::HashSet<String> = code
        .iter()
        .flat_map(|p| read(p).split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map(str::to_string).collect::<Vec<_>>())
        .collect();
    let mut pages = vec![root.join("README.md")];
    for dir in ["docs/engineering", "docs/manual"] {
        sources(&root.join(dir), &[".md"], &mut pages);
    }
    let mut wrong = vec![];
    let (mut names, mut cited) = (0, 0);
    for page in &pages {
        let text = read(page);
        for (i, span) in text.split('`').enumerate() {
            let name = i % 2 == 1
                && span.matches('_').count() >= 3
                && span.starts_with(|c: char| c.is_ascii_lowercase())
                && span.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if name {
                names += 1;
                if !words.contains(span) {
                    wrong.push(format!("{} names `{span}`, which no code has", shown(page)));
                }
            }
        }
    }
    let rust = code.iter().filter(|p| p.extension().is_some_and(|e| e == "rs"));
    for file in rust.chain(&pages) {
        let text = read(file);
        let path_char = |c: char| c.is_ascii_alphanumeric() || "_./-".contains(c);
        for (at, _) in text.match_indices("docs/") {
            if text[..at].chars().next_back().is_some_and(path_char) {
                continue;
            }
            let end = text[at..].find(|c: char| !path_char(c)).map_or(text.len(), |n| at + n);
            let named = text[at..end].trim_end_matches('.');
            if named.ends_with(".md") {
                cited += 1;
                if !root.join(named).is_file() {
                    wrong.push(format!("{} names {named}, which isn't there", shown(file)));
                }
            }
        }
    }
    wrong.sort();
    wrong.dedup();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(names >= 500 && cited >= 100, "only {names} test names and {cited} docs pages found named: is the scan reading the pages?");
}

/// Whether `src` waits in an action's body: `await event(…)` or `await approval(…)`.
fn waits(src: &str) -> bool {
    src.contains("await event(") || src.contains("await approval(")
}

/// A flow with each kind of step after a wait that uses nothing of it: a send, a request, a state
/// write (the key written before the wait too), a collection's and a database's write, another
/// action started, a log line, a send in a branch, a request bound in a branch of a value, then a
/// second wait, a send after it and the answer. The samples' steps mostly use what they waited
/// for, or follow a return that tests it, and wait for it that way; these wait only by being
/// written after it.
const EVERY_STEP_AFTER_A_WAIT: &str = r#"import { action, event, flow, route } from "holon"
import state from "state"
import notes from "collection:notes"
import shop from "database:shop"
import updates from "channel:updates"
import api from "host:api.example.com"

flow("waits", { port: 8999 })

route.post("/start", (req) => {
  const started = job(req.json)
  return started.waiting.go
})

route.post("/go/:token", () => "ok", { resumes: "go" })

route.post("/next/:token", () => "ok", { resumes: "next" })

export const mark = action((m) => {
  state.marked = m
})

export const job = action(async (asked) => {
  state.at = "asked"
  const go = await event("go", { expires: "1d" })
  updates.send({ id: asked.id })
  const sent = await api.post("/x", { body: "after go" })
  state.at = asked.id
  state.via = go.method
  notes[asked.id] = { status: sent.status }
  shop.exec("insert into seen (id) values ($1)", asked.id)
  mark({ by: "job" })
  console.warn("went on")
  if (asked.loud) {
    updates.send({ loud: true })
  }
  const checked = asked.check ? await api.get("/check") : undefined
  const next = await event("next", { expires: "1d" })
  updates.send({ id: asked.id, checked, next: next.method })
  return { done: asked.id }
})
"#;

/// A flow whose action, after its wait, starts another action and calls out to a host, both with
/// nothing of the wait (the review's R138: before the tie, both were asked for while the run
/// still waited).
const NOTHING_OF_THE_WAIT_AFTER_IT: &str = r#"import { action, event, flow, route } from "holon"
import far from "host:127.0.0.1:18307"
import marks from "collection:marks"
import state from "state"

flow("afterwait", { port: 8998 })

route.post("/one/:id", (req) => one({ id: req.params.id }).waiting)

route.get("/go/:token", () => "went", { resumes: "go" })

export const mark = action((m) => {
  marks[m.id] = m
})

export const one = action(async (input) => {
  const go = await event("go", { expires: "1h" })
  mark({ id: input.id })
  await far.post(`/hit/one/${input.id}`, { body: "one" })
  state.after_one = go.method ?? "one went on"
})
"#;

/// Each text that waits, with the name a failure gives it: the samples that do, the README's
/// and the manual's flows that do, LANGUAGE.md's that compile, [`EVERY_STEP_AFTER_A_WAIT`] and
/// [`NOTHING_OF_THE_WAIT_AFTER_IT`].
fn waiting_texts() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = samples::samples().iter().filter(|s| waits(s.text)).map(|s| (format!("sample {}", s.name), s.text.to_string())).collect();
    out.push(("every step after a wait".to_string(), EVERY_STEP_AFTER_A_WAIT.to_string()));
    out.push(("nothing of the wait after it".to_string(), NOTHING_OF_THE_WAIT_AFTER_IT.to_string()));
    let language = std::fs::read_to_string(manual().join("../LANGUAGE.md")).unwrap().replace("\r\n", "\n");
    for (name, text) in docs().into_iter().chain([("docs/LANGUAGE.md".to_string(), language)]) {
        for block in fences(&text) {
            if block.lang == "ts" && waits(&block.body) && holon_author::lang::compile(&block.body).is_ok() {
                out.push((format!("{name}:{}", block.line), block.body));
            }
        }
    }
    out
}

/// `src` compiled and printed from its graph, as the canvas's text is.
fn printed_from_graph(src: &str) -> Result<(holon_author::lang::Lowered, String), String> {
    let l = holon_author::lang::compile(src).map_err(|e| e.to_string())?;
    let text = holon_author::lang::print(l.name.as_deref(), &l.doc, &mut holon_core::MapResolver::new(), &BTreeMap::new(), &l.names)?;
    Ok((l, text))
}

/// Each wait in `l` (compiled from `text`, as `holon fmt` lays it out) with what its action asks
/// for after it, up to the next wait: each node a statement there places that takes a trigger (a
/// write, a send, a request, a call, a start, a log line), and the action's answer.
fn after_each_wait<'a>(l: &'a holon_author::lang::Lowered, text: &str) -> Vec<(&'a holon_author::canvas::Node, Vec<&'a holon_author::canvas::Node>)> {
    let lines: Vec<&str> = text.lines().collect();
    let asks = |n: &holon_author::canvas::Node| n.data.inputs.iter().any(|p| p.name == holon_core::TRIGGER) || n.is_template("respond");
    let waits: Vec<&holon_author::canvas::Node> = l.doc.nodes.iter().filter(|n| matches!(n.data.name.as_str(), "await" | "approval")).collect();
    let mut out = vec![];
    for wait in &waits {
        let Some(at) = l.spans.get(&wait.id) else { continue };
        // The action's body it is in: from its line up to its closing `})`, as fmt writes it, or
        // to the next wait in it.
        let from = at.end.line as usize;
        let end = lines.iter().enumerate().skip(from).find(|(_, l)| **l == "})").map_or(lines.len(), |(i, _)| i + 1);
        let next = waits.iter().filter_map(|w| l.spans.get(&w.id)).map(|s| s.start.line as usize).filter(|&n| n > from && n <= end).min();
        let to = next.map_or(end, |n| n - 1);
        let later = l.doc.nodes.iter().filter(|n| n.id != wait.id && asks(n) && l.spans.get(&n.id).is_some_and(|s| (s.start.line as usize) > from && (s.start.line as usize) <= to)).collect();
        out.push((*wait, later));
    }
    out
}

/// The nodes `from` reaches through the graph's edges, itself among them.
fn reached<'a>(l: &'a holon_author::lang::Lowered, from: &'a str) -> BTreeSet<&'a str> {
    let mut reach: BTreeSet<&str> = BTreeSet::from([from]);
    let mut todo = vec![from];
    while let Some(id) = todo.pop() {
        for e in l.doc.edges.iter().filter(|e| e.source == id) {
            if reach.insert(e.target.as_str()) {
                todo.push(e.target.as_str());
            }
        }
    }
    reach
}

/// Whether node `id` is made only once what `reach` holds (what a wait reaches) has a value:
/// every way its value goes on enters a guard's value whose condition the wait reaches. A guard
/// reads its condition first and its value only when that is true, so a call in a branch of a
/// value held to the wait (`const checked = c ? await api.get(…) : undefined`, lower.rs `stmt`'s
/// `held`) is made only after the wait, though nothing of the wait feeds its own trigger.
fn held(l: &holon_author::lang::Lowered, id: &str, reach: &BTreeSet<&str>) -> bool {
    let outs: Vec<_> = l.doc.edges.iter().filter(|e| e.source == id).collect();
    let guard = |to: &str| l.doc.nodes.iter().any(|n| n.id == to && n.data.name == "guard");
    let on = |to: &str| l.doc.edges.iter().any(|c| c.target == to && c.input() == "cond" && reach.contains(c.source.as_str()));
    !outs.is_empty() && outs.iter().all(|e| (guard(&e.target) && e.input() == "value" && on(&e.target)) || held(l, &e.target, reach))
}

/// Guards: in an action's body, what is written after `await event(…)` or `await approval(…)` is
/// asked for once the wait is answered, as in JavaScript, and the text says so with nothing
/// added. Each text that waits builds as a deploy builds it, and printed from its graph, prints
/// back as it is; and in that graph
/// every write, send, request, call and start a statement after a wait places (up to the next
/// wait, as two waits in a row still park together), and the action's answer, is reached from
/// the wait: it takes its trigger from it, or is held behind a guard on it (`held`), so the host
/// is asked for it only in the part of the run that goes on from the wait.
/// Why: before the tie, a step after a wait that used nothing of it was asked for while the run
/// still waited (the review's R138: a host called, another action started, before the event came).
/// If it fails: a statement kind ts.rs doesn't tie to the wait (`Reader::waited`), or a shape
/// print.rs doesn't read back as plain order (`without_block_ties`). Tie it there, for every
/// kind at once, not for the text that failed.
#[test]
fn what_an_action_asks_for_after_a_wait_waits_for_it_and_prints_back_as_written() {
    let texts = waiting_texts();
    assert!(texts.iter().filter(|(at, _)| at.starts_with("sample ")).count() >= 5 && texts.len() >= 8, "too few texts that wait: {:?}", texts.iter().map(|(at, _)| at).collect::<Vec<_>>());
    let mut wrong = vec![];
    let mut checked = 0;
    for (at, src) in &texts {
        // Its units hash, as `holon check` and a deploy build them: the holds of two waits read by
        // nothing would be the same steps twice (EncodeError::UnusedTwins).
        if let Err(e) = holon_author::agent::plan(src) {
            wrong.push(format!("{at}: doesn't build as a deploy would: {e}"));
            continue;
        }
        let (_, text) = printed_from_graph(src).unwrap_or_else(|e| panic!("{at}: {e}"));
        let (l, again) = printed_from_graph(&text).unwrap_or_else(|e| panic!("{at}: printed, it doesn't compile: {e}\n{text}"));
        if again != text {
            wrong.push(format!("{at}: printed from its graph, it prints back otherwise:\n{text}\n--- then ---\n{again}"));
            continue;
        }
        for (wait, later) in after_each_wait(&l, &text) {
            let reach = reached(&l, &wait.id);
            for n in later {
                checked += 1;
                if !reach.contains(n.id.as_str()) && !held(&l, &n.id, &reach) {
                    let line = l.spans[&n.id].start.line as usize;
                    wrong.push(format!("{at}: {} on line {line} is written after the wait {:?} but doesn't wait for it: `{}`", n.data.name, wait.param("name"), text.lines().nth(line - 1).unwrap_or("").trim()));
                }
            }
        }
    }
    assert!(checked >= 20, "only {checked} steps after a wait checked: is the scan finding them?");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Guards: a start of another action and a call out to a host, written after a wait and using
/// nothing of it, are asked for only once the wait is answered: both take their trigger from the
/// wait, through the graph.
/// Why: the review found both made while the run still waited at the event (R138), the host
/// called before anyone answered; nothing of the wait reached them, so nothing held them back.
/// If it fails: the start or the request lost its tie to the wait (ts.rs `Reader::trigger`).
#[test]
fn a_start_and_a_call_out_after_a_wait_using_nothing_of_it_wait_for_it() {
    let l = holon_author::lang::compile(NOTHING_OF_THE_WAIT_AFTER_IT).unwrap_or_else(|e| panic!("{e}"));
    let wait = l.doc.nodes.iter().find(|n| n.data.name == "await").expect("the wait");
    let reach = reached(&l, &wait.id);
    let line = l.spans[&wait.id].start.line;
    let after: BTreeMap<&str, bool> = l
        .doc
        .nodes
        .iter()
        .filter(|n| holon_core::CALLS.contains(&n.data.name.as_str()) && l.spans.get(&n.id).is_some_and(|s| s.start.line > line))
        .map(|n| (n.data.name.as_str(), reach.contains(n.id.as_str())))
        .collect();
    assert_eq!(after, BTreeMap::from([("flow_start", true), ("http_request", true)]), "each call after the wait, and whether the wait reaches it");
}

/// Guards: a run reaches every wait of its action at once. Each action of each text that waits,
/// built as a server runs it and run with nothing delivered, asks for every wait its text has:
/// two waits in a row park together, as the docs say ("the run waits at both from the start").
/// Why: a hold reading the hold before (ts.rs `waited_on`) reached the first wait only once the
/// second was answered, as a guard reads its value once its test holds: the run parked at the
/// second alone, and the first's token found no run waiting (tests.rs
/// a_waiting_run_is_sealed_once…, 2026-11-19).
/// If it fails: something written after a wait holds an earlier wait behind a guard on a later
/// one. Hold it where every wait is read at once, as the action's hold's list does.
#[test]
fn a_run_reaches_every_wait_of_its_action_at_once() {
    use holon_core::{Answer, Value};
    // Every wait asked for is left waiting; a database query is answered with a row holding each
    // column it selects (as tests.rs's expiry test answers it), as what a wait is asked with may
    // read one; any other read, nothing.
    struct Parked(BTreeSet<String>);
    impl holon_core::Observer for Parked {
        fn answer(&mut self, _path: &[u32], request: &Value) -> Answer {
            let Value::Record(r) = request else { return Answer::Absent };
            let text = |key: &str| match r.get(key) {
                Some(Value::Text(t)) => Some(t.to_lowercase()),
                _ => None,
            };
            if let Some(name) = r.get("await").and_then(|n| if let Value::Text(n) = n { Some(n.clone()) } else { None }) {
                self.0.insert(name);
                return Answer::NotYet;
            }
            if let (Some(_), Some(sql)) = (text("database"), text("query"))
                && let Some(columns) = sql.strip_prefix("select ").and_then(|s| s.split(" from ").next())
            {
                let row = Value::Record(columns.split(',').map(|c| (c.trim().rsplit([' ', '.']).next().unwrap_or_default().to_string(), Value::Number(1.0))).collect());
                return Answer::Value(if r.get("one") == Some(&Value::Boolean(true)) { row } else { Value::List(vec![row]) });
            }
            Answer::Absent
        }
    }
    // Every field the waiting texts' actions read of what starts them (tests.rs's expiry test's).
    let input = holon_core::parse_json(r#"{"id":"p1","item":"tea","amount":5000,"iban":"DE89370400440532013000","by":"sam@acme.com","account":1,"title":"Hello","brief":"A short note","product":"BTC-USD","price":"100","size":"1","side":"buy","n":1,"pad":"p"}"#).unwrap();
    let mut wrong = vec![];
    let mut two = 0;
    for (at, src) in waiting_texts() {
        let lowered = holon_author::lang::compile(&src).unwrap_or_else(|e| panic!("{at}: {e}"));
        let written: BTreeSet<String> = lowered.doc.nodes.iter().filter(|n| matches!(n.data.name.as_str(), "await" | "approval")).map(|n| n.param("name").to_string()).collect();
        let mut store = holon_core::MapResolver::new();
        holon_author::text::restore(&lowered.doc, &mut store, &BTreeMap::new());
        let port = lowered.doc.server.as_ref().map(|s| s.port.clone()).unwrap_or_default();
        let server = holon_author::flow::derive_server(&lowered.doc.nodes, &lowered.doc.edges, &mut store, &port, lowered.doc.unit_settings());
        let mut reached = Parked(BTreeSet::new());
        for action in &server.actions {
            let unit = holon_core::Resolver::resolve(&store, &holon_core::parse_hash(&action.hash).unwrap()).unwrap_or_else(|| panic!("{at}: {}'s unit", action.label)).clone();
            holon_core::eval_observed(&unit, &[Some(input.clone())], &store, &mut reached);
        }
        two += usize::from(written.len() >= 2);
        if reached.0 != written {
            wrong.push(format!("{at}: written {written:?}, reached with nothing delivered {:?}", reached.0));
        }
    }
    assert!(two >= 1, "no text waits twice: the test needs one that does");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
