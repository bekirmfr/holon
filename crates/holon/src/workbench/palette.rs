//! Ctrl+P: everything there is to open and do, found by typing a few letters of it.

use holon_author::samples;

use super::app::{App, Modal, Page, Show, PAGES};
use super::flow::Section;

/// What picking an entry does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Act {
    Page(Page),
    Flow(String, Section),
    Edit(String),
    Call(String),
    Sample(String),
    Tab(usize),
    /// A tab's graph.
    Graph(usize),
    /// The admin page, at one of its parts.
    Admin(crate::workbench::admin::Part),
    /// The Approvals page's runs waiting, at this run (by its id) when one is named.
    Waiting(Option<String>),
    /// The Approvals page's deploys waiting, at this one (its place in the list).
    Deploy(usize),
    /// The Approvals page's access changes, at this one (by its id).
    Change(u64),
    /// A flow's run, by its number.
    Run(String, u64),
    /// As if typed after `:`.
    Command(String),
    Help,
}

#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub(crate) label: String,
    /// Said beside it, dimmed: what kind of thing it is.
    pub(crate) kind: &'static str,
    pub(crate) act: Act,
}

fn item(label: impl Into<String>, kind: &'static str, act: Act) -> Item {
    Item { label: label.into(), kind, act }
}

/// Everything, in the order shown before anything is typed.
fn all(app: &App) -> Vec<Item> {
    let mut out = vec![];
    if let Some(tab) = app.tabs.get(app.tab) {
        let name = &tab.name;
        out.push(item(format!("Deploy {name}"), "this tab", Act::Command("deploy".into())));
        out.push(item(format!("Save {name}"), "this tab", Act::Command("w".into())));
        out.push(item(format!("Format {name}"), "this tab", Act::Command("fmt".into())));
        out.push(item(format!("Graph of {name}"), "this tab", Act::Graph(app.tab)));
        out.push(item(format!("Close {name}"), "this tab", Act::Command("close".into())));
    }
    if let Some(d) = &app.detail {
        let s = &d.slug;
        out.push(item(format!("Call {s}"), "this flow", Act::Call(s.clone())));
        out.push(item(format!("Go back to an earlier point of {s}"), "this flow", Act::Flow(s.clone(), Section::History)));
        out.push(item(format!("Run {s}"), "this flow", Act::Command(format!("run {s}"))));
        out.push(item(format!("Pause {s}"), "this flow", Act::Command(format!("pause {s}"))));
        out.push(item(format!("Stop {s}"), "this flow", Act::Command(format!("stop {s}"))));
        out.push(item(format!("Sessions of {s}"), "this flow", Act::Command(format!("sessions {s}"))));
    }
    for (page, name) in PAGES {
        if page != Page::Flow {
            out.push(item(name, "page", Act::Page(page)));
        }
    }
    for (i, t) in app.tabs.iter().enumerate() {
        out.push(item(t.name.clone(), "tab", Act::Tab(i)));
    }
    out.push(item("Runs waiting", "approvals", Act::Waiting(None)));
    for r in app.runs_waiting().iter().filter(|r| crate::workbench::approvals::yours(r).is_some()) {
        let (slug, id) = (r["slug"].as_str().unwrap_or("?"), r["run_id"].as_str().unwrap_or("?"));
        out.push(item(format!("Approve run {} of {slug}", crate::decidecmd::short(id)), "approvals", Act::Waiting(Some(id.to_string()))));
    }
    // The deploys waiting for a person, and the access changes theirs to approve.
    for (i, (slug, wait)) in app.waiting().into_iter().enumerate() {
        out.push(item(format!("{slug} waits {}", crate::workbench::record::cut(&wait, 60)), "approvals", Act::Deploy(i)));
    }
    if let Some(c) = app.client.as_ref().filter(|c| c.may("people.assign", None).is_ok()) {
        let me = c.me.clone().unwrap_or_default();
        for p in app.changes_waiting().iter().filter(|p| crate::workbench::approvals::not_yours(p, &me).is_none()) {
            let id = p["id"].as_u64().unwrap_or(0);
            out.push(item(format!("Approve access change #{id}: {}", p["what"].as_str().unwrap_or("?")), "approvals", Act::Change(id)));
        }
    }
    if let Some(c) = &app.client {
        for d in &c.deployments {
            let Some(s) = d["slug"].as_str() else { continue };
            out.push(item(s, "flow", Act::Flow(s.into(), Section::Status)));
            out.push(item(format!("{s} runs"), "flow", Act::Flow(s.into(), Section::Runs)));
            out.push(item(format!("{s} logs"), "flow", Act::Flow(s.into(), Section::Logs)));
            out.push(item(format!("{s} history"), "flow", Act::Flow(s.into(), Section::History)));
            out.push(item(format!("{s} state"), "flow", Act::Flow(s.into(), Section::State)));
            out.push(item(format!("{s} access"), "flow", Act::Flow(s.into(), Section::Access)));
            out.push(item(format!("{s} graph"), "flow", Act::Flow(s.into(), Section::Graph)));
            out.push(item(format!("Edit {s}"), "flow", Act::Edit(s.into())));
            out.push(item(format!("Call {s}"), "flow", Act::Call(s.into())));
        }
        for f in &c.flows {
            let Some(n) = f["name"].as_str() else { continue };
            if c.deployment(n).is_none() {
                out.push(item(format!("Edit {n}"), "saved", Act::Edit(n.into())));
            }
        }
    }
    // The latest runs the server's summary names, each to open.
    if let Some(c) = &app.client {
        for l in c.summary["latest"].as_array().into_iter().flatten() {
            let (Some(slug), Some(seq)) = (l["slug"].as_str(), l["seq"].as_u64()) else { continue };
            let kind = if l["failed"] == true { "run, failed" } else if l["waited"] == true { "run, waited" } else { "run" };
            out.push(item(format!("run #{seq} of {slug}"), kind, Act::Run(slug.into(), seq)));
        }
    }
    out.push(item("New draft", "editor", Act::Command("new".into())));
    for s in samples::samples() {
        out.push(item(format!("New from {}", s.name), "sample", Act::Sample(s.name.into())));
    }
    out.push(item("Read the server again", "server", Act::Command("refresh".into())));
    for (part, name) in crate::workbench::admin::PARTS {
        out.push(item(format!("Admin: {name}"), "admin", Act::Admin(part)));
    }
    out.push(item("Run a shell command", "shell", Act::Command("!".into())));
    out.push(item("Keys", "help", Act::Help));
    out.push(item("Quit", "", Act::Command("q".into())));
    out
}

/// How well `query` matches `label`: its words in it (better at a word's start), else its
/// letters in order; none when it doesn't.
fn score(label: &str, query: &str) -> Option<u32> {
    let label = label.to_lowercase();
    let mut total = 0;
    for word in query.to_lowercase().split_whitespace() {
        total += match label.find(word) {
            Some(0) => 0,
            Some(i) if !label.as_bytes()[i - 1].is_ascii_alphanumeric() => 1,
            Some(_) => 3,
            None => {
                let mut rest = label.chars();
                if !word.chars().all(|c| rest.any(|l| l == c)) {
                    return None;
                }
                8
            }
        };
    }
    Some(total)
}

/// The run a query asks for by its number (`4812`, `#4812`, `run 4812`), as the web's does.
fn run_number(query: &str) -> Option<u64> {
    let q = query.trim();
    let q = q.strip_prefix("run").or_else(|| q.strip_prefix("Run")).map_or(q, |r| if r.starts_with(char::is_whitespace) { r.trim_start() } else { q });
    let q = q.strip_prefix('#').unwrap_or(q);
    (!q.is_empty() && q.bytes().all(|b| b.is_ascii_digit())).then(|| q.parse().ok()).flatten()
}

/// The entries that match, best first; all of them for an empty query. A run asked for by its
/// number comes first: that run of each deployed flow (an entry it gives isn't shown twice).
pub(crate) fn items(app: &App, query: &str) -> Vec<Item> {
    let first: Vec<Item> = match (run_number(query), &app.client) {
        (Some(seq), Some(c)) => c.deployments.iter().filter_map(|d| d["slug"].as_str()).map(|s| item(format!("run #{seq} of {s}"), "run", Act::Run(s.into(), seq))).collect(),
        _ => vec![],
    };
    let mut scored: Vec<(u32, usize, Item)> = all(app).into_iter().enumerate().filter(|(_, it)| !first.iter().any(|f| f.label == it.label)).filter_map(|(i, it)| score(&it.label, query).map(|s| (s, i, it))).collect();
    scored.sort_by_key(|(s, i, _)| (*s, *i));
    first.into_iter().chain(scored.into_iter().map(|(_, _, it)| it)).collect()
}

pub(crate) fn run(app: &mut App, act: &Act) {
    match act {
        Act::Page(p) => app.go(*p),
        Act::Flow(slug, section) => {
            app.open_flow(slug);
            if app.page == Page::Flow {
                app.flow_section(*section);
            }
        }
        Act::Edit(name) => app.edit(name),
        Act::Call(slug) => app.call(slug),
        Act::Sample(name) => app.open_sample(name),
        Act::Tab(i) => {
            app.show_tab(*i);
            app.typing = app.tabs[*i].runs.is_none() && app.show != Show::Graph;
        }
        Act::Graph(i) => {
            app.show_tab(*i);
            if app.show == Show::Code {
                app.set_show(Show::Both);
            }
        }
        Act::Admin(p) => {
            app.go(Page::Admin);
            app.admin_part(*p);
        }
        Act::Command(c) => app.run_command(c),
        Act::Help => app.modal = Some(Modal::Help { scroll: 0 }),
        Act::Waiting(id) => app.show_waiting_run(id.as_deref()),
        Act::Deploy(i) => {
            app.go(Page::Approvals);
            app.approvals_part = crate::workbench::approvals::Part::Deploys;
            app.sel[1] = *i;
        }
        Act::Change(id) => {
            app.go(Page::Approvals);
            app.approvals_part = crate::workbench::approvals::Part::Access;
            app.change_sel = app.changes_waiting().iter().position(|p| p["id"].as_u64() == Some(*id)).unwrap_or(0);
        }
        Act::Run(slug, seq) => app.open_run(slug, *seq),
    }
}

#[cfg(test)]
mod tests {
    use super::{run_number, score};

    #[test]
    fn a_run_is_asked_for_by_its_number() {
        for q in ["4812", "#4812", "run 4812", " run #4812 ", "Run 4812"] {
            assert_eq!(run_number(q), Some(4812), "{q}");
        }
        for q in ["", "run", "runs 4", "4812x", "bump 4", "#"] {
            assert_eq!(run_number(q), None, "{q}");
        }
    }

    #[test]
    fn words_match_best_at_their_start_and_letters_in_order_last() {
        assert_eq!(score("Deploy bump", "dep"), Some(0));
        assert_eq!(score("bump history", "his"), Some(1));
        assert!(score("bump history", "bh").is_some());
        assert!(score("bump history", "x").is_none());
        assert!(score("bump history", "his").unwrap() < score("bump history", "bh").unwrap());
    }
}
