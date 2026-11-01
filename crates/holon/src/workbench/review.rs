//! A change looked at before it is made (a deploy to approve, a restore's dry run): everything it
//! changes, in hunks that open and close. Its code line by line, numbered, the unchanged runs far
//! from a change folded, the names a new permission comes from marked; then what it touches,
//! group by group; then its state. A hunk with nothing new in it starts closed.

use std::collections::BTreeSet;

use holon_author::editing::{self, Class};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;

use super::app::{Said, Tone};
use super::diff;
use crate::touches::Mark;

/// Unchanged lines kept either side of a change; a longer run between changes folds.
const AROUND: usize = 3;

/// A line of code: kept (' '), taken out ('-') or put in ('+'); its number (in the text before
/// for one taken out, after for the rest); its colours, and the names in it a permission comes
/// from, in characters.
#[derive(Clone, Debug)]
pub(crate) struct Code {
    pub(crate) kind: char,
    pub(crate) number: usize,
    pub(crate) text: String,
    pub(crate) colours: Vec<(usize, usize, Class)>,
    pub(crate) marked: Vec<(usize, usize)>,
}

#[derive(Debug)]
pub(crate) enum Body {
    Code(Vec<Code>),
    Lines(Vec<Said>),
}

#[derive(Debug)]
pub(crate) struct Hunk {
    pub(crate) head: String,
    /// What is in it, in a word or two each, coloured: `+4 −1`, `2 new · 1 not ready`.
    pub(crate) sum: Vec<Said>,
    pub(crate) body: Body,
    pub(crate) open: bool,
    /// The folds opened: by the first line of each.
    pub(crate) unfolded: BTreeSet<usize>,
}

impl Hunk {
    pub(crate) fn lines(head: impl Into<String>, sum: Vec<Said>, lines: Vec<Said>, open: bool) -> Hunk {
        Hunk { head: head.into(), sum, body: Body::Lines(lines), open, unfolded: BTreeSet::new() }
    }
}

/// A row as drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Row {
    /// One of the lines said before the hunks (`top`) or after them (`foot`).
    Top(usize),
    Foot(usize),
    Blank,
    Head(usize),
    /// Hunk, its first folded line, how many.
    Fold(usize, usize, usize),
    Code(usize, usize),
    Line(usize, usize),
}

/// A place the focus stops at: a hunk's head, or a fold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Stop {
    Head(usize),
    Fold(usize, usize),
}

#[derive(Debug, Default)]
pub(crate) struct Review {
    pub(crate) top: Vec<Said>,
    pub(crate) hunks: Vec<Hunk>,
    pub(crate) foot: Vec<Said>,
    /// Every hunk open and nothing folded, whatever each was.
    pub(crate) whole: bool,
    /// The stop with the focus.
    pub(crate) at: usize,
    pub(crate) scroll: usize,
    /// How many rows showed when last drawn: what moving the focus keeps it in.
    pub(crate) height: usize,
}

impl Review {
    pub(crate) fn rows(&self) -> Vec<Row> {
        let mut out: Vec<Row> = (0..self.top.len()).map(Row::Top).collect();
        for (h, hunk) in self.hunks.iter().enumerate() {
            if !out.is_empty() {
                out.push(Row::Blank);
            }
            out.push(Row::Head(h));
            if !(hunk.open || self.whole) {
                continue;
            }
            match &hunk.body {
                Body::Lines(lines) => out.extend((0..lines.len()).map(|i| Row::Line(h, i))),
                Body::Code(code) => {
                    let near = near_a_change(code);
                    let mut i = 0;
                    while i < code.len() {
                        let run = (i..code.len()).take_while(|&j| !near[j]).count();
                        if run >= 2 && !self.whole && !hunk.unfolded.contains(&i) {
                            out.push(Row::Fold(h, i, run));
                            i += run;
                        } else {
                            out.push(Row::Code(h, i));
                            i += 1;
                        }
                    }
                }
            }
        }
        if !self.foot.is_empty() {
            out.push(Row::Blank);
            out.extend((0..self.foot.len()).map(Row::Foot));
        }
        out
    }

    pub(crate) fn stops(rows: &[Row]) -> Vec<(Stop, usize)> {
        rows.iter()
            .enumerate()
            .filter_map(|(i, r)| match *r {
                Row::Head(h) => Some((Stop::Head(h), i)),
                Row::Fold(h, from, _) => Some((Stop::Fold(h, from), i)),
                _ => None,
            })
            .collect()
    }

    /// The stop with the focus, and its row.
    pub(crate) fn focus(&self) -> Option<(Stop, usize)> {
        let stops = Review::stops(&self.rows());
        stops.get(self.at.min(stops.len().saturating_sub(1))).copied()
    }

    /// The focus to the next stop (or the one before), its row brought into view.
    pub(crate) fn step(&mut self, by: isize) {
        let stops = Review::stops(&self.rows());
        if stops.is_empty() {
            return;
        }
        self.at = (self.at as isize + by).rem_euclid(stops.len() as isize) as usize;
        self.show(stops[self.at].1);
    }

    fn show(&mut self, row: usize) {
        let h = self.height.max(1);
        if row < self.scroll || row >= self.scroll + h {
            self.scroll = row.saturating_sub(2);
        }
    }

    /// Open or close the hunk or the fold with the focus (`to`: open it, close it, or the other).
    pub(crate) fn toggle(&mut self, to: Option<bool>) {
        let Some((stop, _)) = self.focus() else { return };
        match stop {
            Stop::Head(h) => {
                let hunk = &mut self.hunks[h];
                hunk.open = to.unwrap_or(!hunk.open);
            }
            // A fold opened stays a stop no more: the focus goes back to its hunk's head.
            Stop::Fold(h, from) if to != Some(false) => {
                self.hunks[h].unfolded.insert(from);
                self.at = Review::stops(&self.rows()).iter().position(|(s, _)| *s == Stop::Head(h)).unwrap_or(0);
            }
            Stop::Fold(..) => {}
        }
        if let Some((_, row)) = self.focus() {
            self.show(row);
        }
    }

    /// Focus the `i`th stop, and open or close it: a click.
    pub(crate) fn click(&mut self, i: usize) {
        self.at = i;
        self.toggle(None);
    }

    /// Everything open and unfolded, or back as each hunk was.
    pub(crate) fn set_whole(&mut self, whole: bool) {
        let focus = self.focus().map(|f| f.0);
        self.whole = whole;
        self.at = focus.and_then(|f| Review::stops(&self.rows()).iter().position(|(s, _)| *s == f)).unwrap_or(0);
        self.scroll = 0;
    }

    /// A key that moves through it, opens or closes a hunk, or shows it whole; false for any other.
    pub(crate) fn key(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Char('n') | KeyCode::Tab => self.step(1),
            KeyCode::Char('N') | KeyCode::Char('p') | KeyCode::BackTab => self.step(-1),
            KeyCode::Char(' ') => self.toggle(None),
            KeyCode::Right => self.toggle(Some(true)),
            KeyCode::Left => self.toggle(Some(false)),
            KeyCode::Char('w') => self.set_whole(!self.whole),
            KeyCode::Char('d') => self.set_whole(false),
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll += 1,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(self.height.max(2) - 1),
            KeyCode::PageDown => self.scroll += self.height.max(2) - 1,
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = usize::MAX / 2,
            _ => return false,
        }
        true
    }

    /// Everything it says, every hunk open and nothing folded, as lines: a code line after its
    /// mark (`+ `, `- `, `  `).
    #[cfg(test)]
    pub(crate) fn said(&self) -> Vec<Said> {
        let mut out = self.top.clone();
        for hunk in &self.hunks {
            let sum: Vec<&str> = hunk.sum.iter().map(|(_, s)| s.as_str()).collect();
            out.push((Tone::Head, if sum.is_empty() { hunk.head.clone() } else { format!("{}  {}", hunk.head, sum.join(" ")) }));
            match &hunk.body {
                Body::Lines(lines) => out.extend(lines.iter().cloned()),
                Body::Code(code) => out.extend(code.iter().map(|c| {
                    let tone = match c.kind {
                        '+' => Tone::Added,
                        '-' => Tone::Removed,
                        _ => Tone::Plain,
                    };
                    (tone, format!("{} {}", if c.kind == ' ' { ' ' } else { c.kind }, c.text))
                })),
            }
        }
        out.extend(self.foot.iter().cloned());
        out
    }
}

/// Whether each line is a change or within `AROUND` lines of one.
fn near_a_change(code: &[Code]) -> Vec<bool> {
    (0..code.len()).map(|i| code[i.saturating_sub(AROUND)..(i + AROUND + 1).min(code.len())].iter().any(|c| c.kind != ' ')).collect()
}

/// The code hunk: `then` to `now`, line by line, coloured, the names `touches` says a change in
/// what it may do comes from marked. None when they are the same text.
pub(crate) fn code(head: impl Into<String>, then: &str, now: &str, touches: &Value) -> Option<Hunk> {
    let lines = diff::lines(then, now);
    let (added, removed) = diff::counts(&lines);
    if added + removed == 0 {
        return None;
    }
    let (then_colours, now_colours) = (colours(then), colours(now));
    let names = names(touches);
    let (mut old, mut new) = (0, 0);
    let code = lines
        .into_iter()
        .map(|(kind, text)| {
            let (number, colours) = if kind == '-' {
                old += 1;
                (old, then_colours.get(old - 1).cloned().unwrap_or_default())
            } else {
                old += usize::from(kind == ' ');
                new += 1;
                (new, now_colours.get(new - 1).cloned().unwrap_or_default())
            };
            let marked = marks(&text, &colours, &names);
            Code { kind, number, text, colours, marked }
        })
        .collect();
    let sum = vec![(Tone::Added, format!("+{added}")), (Tone::Removed, format!("−{removed}"))];
    Some(Hunk { head: head.into(), sum, body: Body::Code(code), open: true, unfolded: BTreeSet::new() })
}

/// A text's colours by line (from 0): (from, to, class), in characters.
pub(crate) fn colours(text: &str) -> Vec<Vec<(usize, usize, Class)>> {
    let mut out: Vec<Vec<(usize, usize, Class)>> = vec![vec![]; text.split('\n').count()];
    for h in editing::highlight(text) {
        if let Some(line) = out.get_mut(h.span.start.line.saturating_sub(1) as usize) {
            let to = if h.span.end.line == h.span.start.line { h.span.end.col } else { u32::MAX };
            line.push((h.span.start.col.saturating_sub(1) as usize, to.saturating_sub(1) as usize, h.class));
        }
    }
    out
}

/// What to look for in the code for each thing whose permission changes or that isn't ready:
/// `secret.api_key`, `state.count`, a host's name, a folder's or a database's (and its tables').
fn names(touches: &Value) -> Vec<String> {
    let mut out = vec![];
    for row in touches.as_array().into_iter().flatten() {
        if row["change"] == "same" && row["ready"].is_null() {
            continue;
        }
        let name = row["name"].as_str().unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        match row["kind"].as_str().unwrap_or_default() {
            "secret" => out.push(format!("secret.{name}")),
            "key" => out.push(format!("state.{name}")),
            "host" | "folder" | "collection" | "flow" | "channel" => out.push(name.to_string()),
            "database" => {
                out.push(name.to_string());
                // Its tables, where a use that changes names one: `write payouts`.
                for u in row["uses"].as_array().into_iter().flatten().filter(|u| u["change"] != "same") {
                    if let Some((_, table)) = u["what"].as_str().unwrap_or_default().split_once(' ') {
                        out.push(table.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Where `names` stand in `line` as whole names, not in a comment.
fn marks(line: &str, colours: &[(usize, usize, Class)], names: &[String]) -> Vec<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let word = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric() || *c == '_');
    let comment = |at: usize| colours.iter().any(|&(from, to, class)| class == Class::Comment && (from..to.max(from + 1)).contains(&at));
    let mut out = vec![];
    for name in names {
        let n: Vec<char> = name.chars().collect();
        for at in 0..chars.len().saturating_sub(n.len() - 1) {
            if chars[at..at + n.len()] == n[..] && !word(at.checked_sub(1).and_then(|i| chars.get(i))) && !word(chars.get(at + n.len())) && !comment(at) {
                out.push((at, at + n.len()));
            }
        }
    }
    out.sort_unstable();
    out
}

/// What a flow touches (a preview's `touches` and `settings`), a hunk a group: its rows coloured
/// by what the change does to each, a note under each; open when something in it changes or
/// isn't ready.
pub(crate) fn touched(preview: &Value) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = vec![];
    let mut counts: Vec<Vec<Mark>> = vec![];
    for l in crate::touches::lines(&preview["touches"], &preview["settings"]) {
        if l.mark == Mark::Head {
            out.push(Hunk::lines(l.text, vec![], vec![], false));
            counts.push(vec![]);
            continue;
        }
        let (Some(hunk), Some(marks)) = (out.last_mut(), counts.last_mut()) else { continue };
        let Body::Lines(lines) = &mut hunk.body else { continue };
        marks.push(l.mark);
        let tone = match l.mark {
            Mark::Head | Mark::Same => Tone::Plain,
            Mark::Approve | Mark::Changed => Tone::Warn,
            Mark::New => Tone::Added,
            Mark::Gone => Tone::Removed,
            Mark::Unready => Tone::Bad,
        };
        lines.push((tone, format!("{}{}", l.mark.sign(), l.text)));
        if !l.note.is_empty() {
            lines.push((if l.mark == Mark::Unready { Tone::Bad } else { Tone::Dim }, format!("    {}", l.note)));
        }
    }
    for (hunk, marks) in out.iter_mut().zip(counts) {
        let n = |m: &[Mark]| marks.iter().filter(|k| m.contains(k)).count();
        let said = [
            (n(&[Mark::Approve]), "to approve", Tone::Warn),
            (n(&[Mark::New]), "new", Tone::Added),
            (n(&[Mark::Changed]), "changed", Tone::Warn),
            (n(&[Mark::Gone]), "no longer", Tone::Removed),
            (n(&[Mark::Unready]), "not ready", Tone::Bad),
            (n(&[Mark::Same]), "the same", Tone::Dim),
        ];
        hunk.open = said[..5].iter().any(|(k, _, _)| *k > 0);
        hunk.sum = said.into_iter().filter(|(k, _, _)| *k > 0).map(|(k, what, tone)| (tone, format!("{k} {what}"))).collect();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn code_folds_far_from_a_change_numbers_its_lines_and_marks_what_a_permission_comes_from() {
        let then = (1..=12).map(|i| format!("a{i} = {i}\n")).collect::<String>();
        let now = then.replace("a10 = 10\n", "a10 = secret.key\n");
        let touches = json!([{ "kind": "secret", "name": "key", "change": "new", "ready": null, "uses": [] }]);
        let hunk = code("Code", &then, &now, &touches).unwrap();
        assert_eq!(hunk.sum, [(Tone::Added, "+1".to_string()), (Tone::Removed, "−1".to_string())]);
        let Body::Code(lines) = &hunk.body else { panic!() };
        let added = lines.iter().find(|c| c.kind == '+').unwrap();
        assert_eq!((added.number, &added.text[..], &added.marked[..]), (10, "a10 = secret.key", &[(6, 16)][..]));
        assert_eq!(lines.iter().find(|c| c.kind == '-').unwrap().number, 10);

        let mut r = Review { hunks: vec![hunk], height: 20, ..Review::default() };
        // Lines 1 to 6 fold; 7 to 12 show, around the change.
        assert_eq!(r.rows()[..3], [Row::Head(0), Row::Fold(0, 0, 6), Row::Code(0, 6)]);
        r.step(1);
        assert_eq!(r.focus().map(|f| f.0), Some(Stop::Fold(0, 0)));
        r.toggle(None);
        assert_eq!(r.rows()[1], Row::Code(0, 0), "the fold opened");
        assert_eq!(r.focus().map(|f| f.0), Some(Stop::Head(0)));
        r.toggle(None);
        assert_eq!(r.rows(), [Row::Head(0)], "the hunk closed");
        r.set_whole(true);
        assert_eq!(r.rows().len(), 1 + 13, "whole: everything open, nothing folded");
    }

    #[test]
    fn a_name_is_marked_whole_and_never_in_a_comment() {
        let names = vec!["inbox".to_string()];
        assert_eq!(marks("x = inbox[name] ?? inboxes", &[], &names), [(4, 9)]);
        assert_eq!(marks("# reads inbox", &[(0, 13, Class::Comment)], &names), []);
    }

    #[test]
    fn a_group_opens_when_something_in_it_changes_and_says_what() {
        let preview = json!({ "touches": [
            { "group": "state", "kind": "key", "name": "n", "change": "same", "uses": [{ "item": "StateWrite(\"n\")", "what": "write", "words": "", "change": "same", "approval": false, "optional": false }], "ready": null, "has": null },
            { "group": "outside", "kind": "host", "name": "b.example", "change": "new", "uses": [{ "item": "HttpRequest(\"b.example\")", "what": "call", "words": "", "change": "new", "approval": true, "optional": false }], "ready": null, "has": null },
        ], "settings": [] });
        let hunks = touched(&preview);
        let heads: Vec<(&str, bool)> = hunks.iter().map(|h| (h.head.as_str(), h.open)).collect();
        assert_eq!(heads, [("What it reaches outside", true), ("What it keeps", false)]);
        assert_eq!(hunks[0].sum, [(Tone::Warn, "1 to approve".to_string())]);
        assert_eq!(hunks[1].sum, [(Tone::Dim, "1 the same".to_string())]);
        let Body::Lines(lines) = &hunks[0].body else { panic!() };
        assert_eq!(lines[..], [(Tone::Warn, "+ host b.example: call".to_string()), (Tone::Dim, "    approved by confirming".to_string())]);
    }
}
