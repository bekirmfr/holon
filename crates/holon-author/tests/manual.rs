//! The docs (the README and docs/manual) stay true to the code: every flow in them (a `ts` block)
//! compiles, as TypeScript, every `holon check` they show is what it prints, and the page of leaves
//! is what the catalog says. The blocks are written to be read, so they needn't be as `holon fmt`
//! writes them. What the engineering docs and the code cite (a test by its name, a docs page by
//! its path) is there.
//!
//! `HOLON_BLESS=1 cargo test -p holon-author --test manual` writes the page of leaves again.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use holon_author::lang::ast::Pos;
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

/// A flow whose action may answer before its wait: where it does, nothing waits; where it doesn't,
/// it waits, and the send after the wait, using nothing of it, waits for it.
const A_RETURN_BEFORE_THE_WAIT: &str = r#"import { action, event, flow, route } from "holon"
import updates from "channel:updates"

flow("early", { port: 8997 })

route.post("/start", (req) => job(req.json).waiting)

route.post("/go/:token", () => "ok", { resumes: "go" })

export const job = action(async (order) => {
  if (order.skip ?? false) return "skipped"
  const go = await event("go", { expires: "1d" })
  updates.send({ sent: order.id })
  return go.method
})
"#;

/// A flow whose action waits at an event and then at an approval, sends what uses neither, and
/// answers with both: a run parks at both at once, and lists them in the order written (the
/// shape of approval_shows.rs's `hold`); the send and the answer wait for both, whichever is
/// answered first.
const AN_EVENT_THEN_AN_APPROVAL: &str = r#"import { action, approval, event, flow, route } from "holon"
import updates from "channel:updates"

flow("twowaits", { port: 8996 })

route.post("/hold", (req) => hold(req.json).waiting.paid)

route.get("/paid/:token", () => "ok", { resumes: "paid" })

export const hold = action(async (order) => {
  const paid = await event("paid", { expires: "1h" })
  const decision = await approval("release", { expires: "1h" })
  updates.send({ released: order.id })
  return [paid, decision.approved]
})
"#;

/// Each text that waits, with the name a failure gives it: the samples that do, the README's
/// and the manual's flows that do, LANGUAGE.md's that compile, [`EVERY_STEP_AFTER_A_WAIT`],
/// [`NOTHING_OF_THE_WAIT_AFTER_IT`], [`A_RETURN_BEFORE_THE_WAIT`] and
/// [`AN_EVENT_THEN_AN_APPROVAL`].
fn waiting_texts() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = samples::samples().iter().filter(|s| waits(s.text)).map(|s| (format!("sample {}", s.name), s.text.to_string())).collect();
    out.push(("every step after a wait".to_string(), EVERY_STEP_AFTER_A_WAIT.to_string()));
    out.push(("nothing of the wait after it".to_string(), NOTHING_OF_THE_WAIT_AFTER_IT.to_string()));
    out.push(("a return before the wait".to_string(), A_RETURN_BEFORE_THE_WAIT.to_string()));
    out.push(("an event then an approval".to_string(), AN_EVENT_THEN_AN_APPROVAL.to_string()));
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
/// back as it is and builds so too (one that answers before its first wait among them, its row
/// of `if (test) return value` printed right above the wait; `a_return_before_the_first_wait_…`
/// runs it); and in that graph
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
    let (mut checked, mut early) = (0, 0);
    for (at, src) in &texts {
        // Its units hash, as `holon check` and a deploy build them: the holds of two waits read by
        // nothing would be the same steps twice (EncodeError::UnusedTwins).
        if let Err(e) = holon_author::agent::plan(src) {
            wrong.push(format!("{at}: doesn't build as a deploy would: {e}"));
            continue;
        }
        let text = printed_from_graph(src).unwrap_or_else(|e| panic!("{at}: {e}")).1;
        if text.lines().any(|line| line.trim_start().starts_with("if (") && line.contains(") return")) && text.contains("await ") {
            early += 1;
        }
        // The printed text builds as a deploy would, as its source does: one that only compiled
        // and printed back the same could still be refused at the deploy.
        if let Err(e) = holon_author::agent::plan(&text) {
            wrong.push(format!("{at}: printed from its graph, it doesn't build as a deploy would: {e}\n{text}"));
            continue;
        }
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
    assert!(early >= 1, "no text that answers before its wait printed so: is {} still among them?", "A_RETURN_BEFORE_THE_WAIT");
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

/// A text's actions, each with its label and its unit, and the text's waits in the order written:
/// the units as the lowering built them, and as a store keeps them. A store keeps a composite's
/// children in their canonical order, by hash (holon-core `canonical_child_order`), which a
/// unit's hash doesn't see, and a run walks the children it starts from in that order: the server
/// runs this form, the editor's tests the first. Both are run.
struct Built {
    waits: Vec<String>,
    actions: Vec<(String, holon_core::Hash)>,
    authored: holon_core::MapResolver,
    stored: holon_core::MapResolver,
}

fn built(at: &str, src: &str) -> Built {
    let lowered = holon_author::lang::compile(src).unwrap_or_else(|e| panic!("{at}: {e}"));
    let mut waits: Vec<(Pos, String)> = lowered
        .doc
        .nodes
        .iter()
        .filter(|n| matches!(n.data.name.as_str(), "await" | "approval"))
        .map(|n| (lowered.spans.get(&n.id).map_or(Pos::default(), |s| s.start), n.param("name").to_string()))
        .collect();
    waits.sort_by_key(|(at, _)| (at.line, at.col));
    let mut authored = holon_core::MapResolver::new();
    holon_author::text::restore(&lowered.doc, &mut authored, &BTreeMap::new());
    let port = lowered.doc.server.as_ref().map(|s| s.port.clone()).unwrap_or_default();
    let server = holon_author::flow::derive_server(&lowered.doc.nodes, &lowered.doc.edges, &mut authored, &port, lowered.doc.unit_settings());
    let mut stored = holon_core::MapResolver::new();
    for unit in authored.units() {
        let kept = holon_core::decode_preimage(&holon_core::encode_preimage(unit).unwrap()).unwrap();
        stored.insert(kept).unwrap();
    }
    let actions = server.actions.iter().map(|a| (a.label.clone(), holon_core::parse_hash(&a.hash).unwrap())).collect();
    let mut names: Vec<String> = vec![];
    for (_, name) in waits {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Built { waits: names, actions, authored, stored }
}

/// A run of an action that waits, as a host answers it: each wait asked for is noted the first
/// time it is, and left waiting, or answered (`answered`, all but `held`); a database query is
/// answered with a row holding each column it selects (as tests.rs's expiry test answers it), as
/// what a wait is asked with may read one; any other read, nothing.
struct Waiting {
    asked: Vec<String>,
    answered: bool,
    held: Option<String>,
}

impl holon_core::Observer for Waiting {
    fn answer(&mut self, _path: &[u32], request: &holon_core::Value) -> holon_core::Answer {
        use holon_core::{Answer, Value};
        let Value::Record(r) = request else { return Answer::Absent };
        let text = |key: &str| match r.get(key) {
            Some(Value::Text(t)) => Some(t.to_lowercase()),
            _ => None,
        };
        if let Some(name) = r.get("await").and_then(|n| if let Value::Text(n) = n { Some(n.clone()) } else { None }) {
            if !self.asked.contains(&name) {
                self.asked.push(name.clone());
            }
            if !self.answered || self.held.as_ref() == Some(&name) {
                return Answer::NotYet;
            }
            // What an event or an approval gives, enough of it for what the texts read.
            return Answer::Value(holon_core::parse_json(r#"{"method":"POST","body":{},"approved":true,"by":["ada"]}"#).unwrap());
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

/// Every field the waiting texts' actions read of what starts them (tests.rs's expiry test's).
fn started_with() -> holon_core::Value {
    holon_core::parse_json(r#"{"id":"p1","order":"o-1","item":"tea","amount":5000,"iban":"DE89370400440532013000","by":"sam@acme.com","account":1,"title":"Hello","brief":"A short note","product":"BTC-USD","price":"100","size":"1","side":"buy","n":1,"pad":"p"}"#).unwrap()
}

/// `action` of `b` in the form `stored` says, run with `input` as `watch` answers it: which of its
/// outputs settled (a value, not pending or absent).
fn run(b: &Built, action: &holon_core::Hash, stored: bool, input: Option<holon_core::Value>, watch: &mut Waiting) -> Vec<bool> {
    let store = if stored { &b.stored } else { &b.authored };
    let unit = holon_core::Resolver::resolve(store, action).expect("the action's unit").clone();
    holon_core::eval_observed(&unit, &[input], store, watch).iter().map(Option::is_some).collect()
}

/// Guards: a run reaches every wait of its action at once, in the order they are written. Each
/// action of each text that waits, built as a server runs it and run with nothing delivered, asks
/// for every wait its text has: two waits in a row park together, as the docs say ("the run waits
/// at both from the start"); and it asks for them in the order written, which is the order a
/// parked run lists them in (`GET …/runs`, the editor's Approvals, `holon runs`): as a store keeps
/// its units and as they were lowered, started with a record and with nothing.
/// Why: a hold reading the hold before (ts.rs `waited_on`) reached the first wait only once the
/// second was answered, as a guard reads its value once its test holds: the run parked at the
/// second alone, and the first's token found no run waiting (tests.rs
/// a_waiting_run_is_sealed_once…, 2026-11-19). And an answer held to the last wait alone asked
/// for it first where the store's order put the answer first: a run waiting at an event and then
/// an approval listed the approval first (approval_shows.rs, found by the long run, 2026-11-20).
/// If it fails: something written after a wait holds an earlier wait behind a guard on a later
/// one, or reads a later wait before the earlier ones. Tie it to every wait so far, read in order
/// (ts.rs `Reader::waited`), as the action's hold's list does.
#[test]
fn a_run_reaches_every_wait_of_its_action_at_once_in_the_order_written() {
    let mut wrong = vec![];
    let mut two = 0;
    for (at, src) in waiting_texts() {
        let b = built(&at, &src);
        let mut reached = BTreeSet::new();
        for (label, action) in &b.actions {
            for stored in [true, false] {
                // Started with a record, and with nothing (`job()`, or a body that isn't JSON):
                // what reads the action's input may then not be read at all.
                for given in [Some(started_with()), None] {
                    let mut watch = Waiting { asked: vec![], answered: false, held: None };
                    run(&b, action, stored, given.clone(), &mut watch);
                    let in_order: Vec<String> = b.waits.iter().filter(|w| watch.asked.contains(w)).cloned().collect();
                    if watch.asked != in_order {
                        let (form, with) = (if stored { "as stored" } else { "as lowered" }, if given.is_some() { "a record" } else { "nothing" });
                        wrong.push(format!("{at}: {label} {form}, started with {with}, asked for its waits as {:?}, written {in_order:?}", watch.asked));
                    }
                    if given.is_some() && stored {
                        two += usize::from(watch.asked.len() >= 2);
                        reached.extend(watch.asked);
                    }
                }
            }
        }
        let written: BTreeSet<String> = b.waits.iter().cloned().collect();
        if reached != written {
            wrong.push(format!("{at}: written {written:?}, reached with nothing delivered {reached:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(two >= 2, "fewer than two actions wait twice: the test needs some that do");
}

/// Guards: what an action writes, sends or answers after two waits in a row waits for both. Each
/// action of each text that waits at two or more, run with every wait answered but the first it
/// writes, settles nothing it doesn't settle with none answered: what follows the first wait waits
/// for it, whatever the later ones gave; as a store keeps its units and as they were lowered.
/// Why: the waits after the first are answered in any order (each has its own token, or its own
/// people), and what was tied to the last wait alone was made once it was answered while the run
/// still waited at the first: a send written after both went out as the run parked again
/// (firing.rs `progress`, found by reading, 2026-11-20).
/// If it fails: something written after the waits is tied to the last one only. Tie it to every
/// wait so far (ts.rs `Reader::waited`).
#[test]
fn what_follows_two_waits_waits_for_both() {
    let mut wrong = vec![];
    let mut checked = 0;
    for (at, src) in waiting_texts() {
        let b = built(&at, &src);
        for (label, action) in &b.actions {
            for stored in [true, false] {
                let mut none = Waiting { asked: vec![], answered: false, held: None };
                let before = run(&b, action, stored, Some(started_with()), &mut none);
                let Some(first) = b.waits.iter().find(|w| none.asked.contains(w)).cloned() else { continue };
                if none.asked.len() < 2 {
                    continue;
                }
                checked += 1;
                let mut rest = Waiting { asked: vec![], answered: true, held: Some(first.clone()) };
                let after = run(&b, action, stored, Some(started_with()), &mut rest);
                let early: Vec<usize> = (0..after.len()).filter(|&i| after[i] && !before.get(i).copied().unwrap_or(false)).collect();
                if !early.is_empty() {
                    let form = if stored { "as stored" } else { "as lowered" };
                    wrong.push(format!("{at}: {label} {form}, every wait answered but {first:?}, settled its outputs {early:?}, which it doesn't with none answered"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(checked >= 4, "only {checked} runs of actions that wait twice: the test needs some");
}

/// Guards: a return before an action's first wait answers without it, whatever follows the wait.
/// Run as a server runs it, an action with two returns above its wait (with values, the first bare,
/// or tests with no default) and each kind of step after it (a send, a state write of the wait's or
/// of nothing of it, a collection write and delete, a write in `if (x !== undefined)`, a request, a
/// branch): started with either return taken, it answers at once and asks for nothing, no wait, no
/// request; started with neither taken, it asks for its wait alone, answers nothing yet and does
/// nothing yet (what follows waits for it); sent neither field, a test with no value ends the run
/// there: nothing asked, nothing answered, so nothing parks it.
/// Why: the answer of an action that waits is held to its waits (ts.rs `finish`); held whole, it
/// would hold the early return too, and a run that returned before its wait would never answer.
/// The action's hold reads the wait: read directly, it asked for the wait in a run that had
/// already answered (found by this test, 2026-11-19). And a write's `after` was tied to the wait
/// alone, so a run that had answered asked for it through the write, and the server parked it
/// there, its answer given (found by the refund-review sample's golden, 2026-11-20).
/// If it fails: the returns before the first wait were folded under the wait's tie (ts.rs
/// `fold`'s `tie`), the hold no longer reads the wait under the returns not taken (ts.rs
/// `waited_on`), or a step after the wait waits for it alone (ts.rs `held`, `trigger`).
#[test]
fn a_return_before_the_first_wait_answers_without_it() {
    use holon_core::{Answer, Value};
    struct Asked(Vec<String>);
    impl holon_core::Observer for Asked {
        fn answer(&mut self, _path: &[u32], request: &Value) -> Answer {
            match request {
                Value::Record(r) => match r.get("await") {
                    Some(Value::Text(name)) => {
                        self.0.push(name.clone());
                        Answer::NotYet
                    }
                    _ => {
                        self.0.push(format!("a request: {:?}", r.keys().collect::<Vec<_>>()));
                        Answer::Absent
                    }
                },
                _ => Answer::Absent,
            }
        }
    }
    let text = |returns: &str, after: &str| {
        let import = |name: &str, line: &str| if after.contains(&format!("{name}.")) || after.contains(&format!("{name}[")) { format!("{line}\n") } else { String::new() };
        format!(
            "import {{ action, event, flow, route }} from \"holon\"\n{}{}{}{}\nflow(\"early\", {{ port: 8997 }})\n\nroute.post(\"/start\", (req) => job(req.json).waiting)\n\nroute.post(\"/go/:token\", () => \"ok\", {{ resumes: \"go\" }})\n\nexport const job = action(async (order) => {{\n{returns}  const go = await event(\"go\", {{ expires: \"1d\" }})\n{after}  return go.method\n}})\n",
            import("state", "import state from \"state\""),
            import("updates", "import updates from \"channel:updates\""),
            import("orders", "import orders from \"collection:orders\""),
            import("api", "import api from \"host:api.example.com\""),
        )
    };
    // The returns: each with what the first answers, and whether a run sent neither field still
    // waits (its tests have a value without them) or ends at the first test, which has none.
    let heads = [
        ("returns with values", "  if (order.skip ?? false) return \"skipped\"\n  if ((order.amount ?? 0) > 100) return { held: true }\n", Some("Text(\"skipped\")"), true),
        ("a bare return", "  if (order.skip ?? false) return\n  if ((order.amount ?? 0) > 100) return { held: true }\n", None, true),
        ("tests with no default", "  if (order.skip) return \"skipped\"\n  if (order.amount > 100) return { held: true }\n", Some("Text(\"skipped\")"), false),
    ];
    let afters = [
        ("a send", "  updates.send({ sent: order.id })\n"),
        ("a state write of the wait's", "  state.last = go.method\n"),
        ("a state write of nothing of the wait", "  state.seen = order.id\n"),
        ("a collection write", "  orders[order.id] = { by: go.method }\n"),
        ("a collection delete", "  delete orders[order.id]\n"),
        ("a write in if (x !== undefined)", "  const m = go.method\n  if (m !== undefined) {\n    state.m = m\n  }\n"),
        ("a request", "  const sent = await api.post(\"/x\", { body: order.id })\n  state.status = sent.status\n"),
        ("a branch", "  if (go.method === \"POST\") updates.send({ posted: order.id })\n"),
    ];
    let mut wrong = Vec::new();
    for ((head, returns, first, waits), (after_case, after)) in heads.iter().flat_map(|h| afters.iter().map(move |a| (h, a))) {
        let case = format!("{head}, then {after_case}");
        let src = text(*returns, *after);
        let lowered = holon_author::lang::compile(&src).unwrap_or_else(|e| panic!("{case}: {e}\n{src}"));
        let mut store = holon_core::MapResolver::new();
        holon_author::text::restore(&lowered.doc, &mut store, &BTreeMap::new());
        let port = lowered.doc.server.as_ref().map(|s| s.port.clone()).unwrap_or_default();
        let server = holon_author::flow::derive_server(&lowered.doc.nodes, &lowered.doc.edges, &mut store, &port, lowered.doc.unit_settings());
        let [action] = server.actions.as_slice() else { panic!("{case}: one action: {:?}", server.actions.iter().map(|a| &a.label).collect::<Vec<_>>()) };
        let unit = holon_core::Resolver::resolve(&store, &holon_core::parse_hash(&action.hash).unwrap()).expect("its unit").clone();
        let run = |input: &str| {
            let mut asked = Asked(vec![]);
            let outputs = holon_core::eval_observed(&unit, &[Some(holon_core::parse_json(input).unwrap())], &store, &mut asked);
            (asked.0, format!("{outputs:?}"))
        };
        for (taken, input, answer) in [("the first return", r#"{"id":"early-7","skip":true}"#, *first), ("the second return", r#"{"id":"early-7","skip":false,"amount":500}"#, Some("held"))] {
            let (asked, outputs) = run(input);
            if !asked.is_empty() {
                wrong.push(format!("{case}, {taken} taken: it asked for {asked:?}"));
            }
            if let Some(answer) = answer
                && !outputs.contains(answer)
            {
                wrong.push(format!("{case}, {taken} taken: it didn't answer {answer}: {outputs}"));
            }
            if outputs.contains("early-7") {
                wrong.push(format!("{case}, {taken} taken: it did what follows the wait: {outputs}"));
            }
        }
        // Neither taken, then sent neither field: a run past every return waits for the wait alone;
        // one whose test has no value ends there, asking for nothing and answering nothing.
        for (given, input, goes_on) in [("neither taken", r#"{"id":"early-7","skip":false,"amount":5}"#, true), ("sent neither field", r#"{"id":"early-7"}"#, *waits)] {
            let (asked, outputs) = run(input);
            let expected: &[&str] = if goes_on { &["go"] } else { &[] };
            if asked != expected {
                wrong.push(format!("{case}, {given}: it asked for {asked:?}, not {expected:?}"));
            }
            if outputs.contains("skipped") || outputs.contains("held") || outputs.contains("early-7") {
                wrong.push(format!("{case}, {given}: it answered, or did what follows the wait, before it: {outputs}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
