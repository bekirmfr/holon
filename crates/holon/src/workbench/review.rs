//! A change looked at before it is made (a deploy to approve, a restore's dry run): everything it
//! changes, in hunks that open and close, each a diff drawn alike. Its code line by line,
//! numbered, the unchanged runs far from a change folded, the names a new permission comes from
//! marked; what it touches, group by group, a row a thing; its state, each value as the code
//! that would set it, taken out and put back. A hunk with nothing new in it starts closed.

use std::collections::BTreeSet;

use holon_author::agent;
use holon_author::editing::{self, Class};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;

use super::app::{Said, Tone};
use super::diff;
use crate::touches::Mark;

/// Unchanged lines kept either side of a change; a longer run between changes folds.
const AROUND: usize = 3;

/// A row of a diff: what the change does to it (`kind`); its number in the text, for code (in the
/// text before for a line taken out, after for the rest); its colours, the names in it a
/// permission comes from, in characters; and a note after it.
///
/// Kinds: ' ' the same, '+' put in or new, 'a' new and approved by confirming, '~' changed,
/// '-' taken out or no longer, '!' not ready on this server; 'i' a line saying what follows, and
/// 'w' one that warns.
#[derive(Clone, Debug, Default)]
pub(crate) struct Code {
    pub(crate) kind: char,
    pub(crate) number: Option<usize>,
    pub(crate) text: String,
    pub(crate) colours: Vec<(usize, usize, Class)>,
    pub(crate) marked: Vec<(usize, usize)>,
    pub(crate) note: String,
}

impl Code {
    fn said(kind: char, text: impl Into<String>) -> Code {
        Code { kind, text: text.into(), ..Code::default() }
    }

    /// The mark it is drawn after.
    pub(crate) fn sign(&self) -> &'static str {
        match self.kind {
            '+' | 'a' => "+ ",
            '-' => "− ",
            '~' => "~ ",
            '!' => "! ",
            _ => "  ",
        }
    }
}

#[derive(Debug)]
pub(crate) struct Hunk {
    pub(crate) head: String,
    /// What is in it, in a word or two each, coloured: `+4 −1`, `1 to approve · 2 the same`.
    pub(crate) sum: Vec<Said>,
    pub(crate) rows: Vec<Code>,
    pub(crate) open: bool,
    /// The folds opened: by the first line of each.
    pub(crate) unfolded: BTreeSet<usize>,
}

impl Hunk {
    pub(crate) fn new(head: impl Into<String>, sum: Vec<Said>, rows: Vec<Code>, open: bool) -> Hunk {
        Hunk { head: head.into(), sum, rows, open, unfolded: BTreeSet::new() }
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
}

#[derive(Debug, Default)]
pub(crate) struct Review {
    pub(crate) top: Vec<Said>,
    pub(crate) hunks: Vec<Hunk>,
    pub(crate) foot: Vec<Said>,
    /// Every hunk open and nothing folded, whatever each was.
    pub(crate) whole: bool,
    /// The row the cursor is on; none until it moves, on the first hunk's head.
    pub(crate) at: Option<usize>,
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
            let near = near_a_change(&hunk.rows);
            let mut i = 0;
            while i < hunk.rows.len() {
                let run = (i..hunk.rows.len()).take_while(|&j| !near[j]).count();
                if run >= 2 && !self.whole && !hunk.unfolded.contains(&i) {
                    out.push(Row::Fold(h, i, run));
                } else {
                    out.extend((i..i + run.max(1)).map(|j| Row::Code(h, j)));
                }
                i += run.max(1);
            }
        }
        if !self.foot.is_empty() {
            out.push(Row::Blank);
            out.extend((0..self.foot.len()).map(Row::Foot));
        }
        out
    }

    /// The rows the cursor jumps between: hunks' heads and folds.
    pub(crate) fn stops(rows: &[Row]) -> Vec<usize> {
        rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::Head(_) | Row::Fold(..))).map(|(i, _)| i).collect()
    }

    /// The row the cursor is on: the first hunk's head until it moves.
    pub(crate) fn cursor(&self, rows: &[Row]) -> usize {
        self.at.or_else(|| rows.iter().position(|r| matches!(r, Row::Head(_)))).unwrap_or(0).min(rows.len().saturating_sub(1))
    }

    fn put(&mut self, row: usize) {
        self.at = Some(row);
        self.show(row);
    }

    /// The cursor a row down (or up), past the blank ones, kept in view.
    pub(crate) fn move_by(&mut self, by: isize) {
        let rows = self.rows();
        let mut at = self.cursor(&rows) as isize;
        let last = rows.len() as isize - 1;
        for _ in 0..by.unsigned_abs() {
            let mut next = at + by.signum();
            while (0..=last).contains(&next) && rows[next as usize] == Row::Blank {
                next += by.signum();
            }
            if !(0..=last).contains(&next) {
                break;
            }
            at = next;
        }
        self.put(at.max(0) as usize);
    }

    /// The cursor to the next hunk or fold (or the one before), round the end.
    pub(crate) fn step(&mut self, by: isize) {
        let rows = self.rows();
        let stops = Review::stops(&rows);
        if stops.is_empty() {
            return;
        }
        let at = self.cursor(&rows);
        self.put(if by > 0 { stops.iter().copied().find(|&s| s > at).unwrap_or(stops[0]) } else { stops.iter().copied().rev().find(|&s| s < at).unwrap_or(stops[stops.len() - 1]) });
    }

    fn show(&mut self, row: usize) {
        let h = self.height.max(1);
        if row < self.scroll {
            self.scroll = row.saturating_sub(1);
        } else if row >= self.scroll + h {
            self.scroll = row + 2 - h.min(row + 2);
        }
    }

    /// Open or close what the cursor is on (`to`: open it, close it, or the other): a hunk's head
    /// opens or closes it, a fold opens, a row inside a hunk closes it, the cursor on its head.
    pub(crate) fn toggle(&mut self, to: Option<bool>) {
        let rows = self.rows();
        let Some(&row) = rows.get(self.cursor(&rows)) else { return };
        match row {
            Row::Head(h) => self.hunks[h].open = to.unwrap_or(!(self.hunks[h].open || self.whole)),
            Row::Fold(h, from, _) if to != Some(false) => {
                self.hunks[h].unfolded.insert(from);
            }
            Row::Fold(h, ..) | Row::Code(h, _) if to != Some(true) => {
                self.hunks[h].open = false;
                self.whole = false;
                self.at = self.rows().iter().position(|r| *r == Row::Head(h));
            }
            _ => {}
        }
        let at = self.cursor(&self.rows());
        self.put(at);
    }

    /// A row clicked: the cursor on it; a hunk's head or a fold opens or closes.
    pub(crate) fn click(&mut self, row: usize) {
        self.at = Some(row);
        if matches!(self.rows().get(row), Some(Row::Head(_) | Row::Fold(..))) {
            self.toggle(None);
        }
    }

    /// Everything open and unfolded, or back as each hunk was; the cursor on the same row, or
    /// the head of its hunk.
    pub(crate) fn set_whole(&mut self, whole: bool) {
        let rows = self.rows();
        let on = rows.get(self.cursor(&rows)).copied();
        self.whole = whole;
        let rows = self.rows();
        let head = |h: usize| rows.iter().position(|r| *r == Row::Head(h));
        let at = match on {
            Some(Row::Code(h, _) | Row::Fold(h, ..)) => rows.iter().position(|r| Some(*r) == on).or_else(|| head(h)),
            Some(r) => rows.iter().position(|x| *x == r),
            None => None,
        }
        .unwrap_or(0);
        self.scroll = 0;
        self.put(at);
    }

    /// A key that moves through it, opens or closes a hunk, or shows it whole; false for any other.
    pub(crate) fn key(&mut self, k: KeyEvent) -> bool {
        let page = self.height.max(2) as isize - 1;
        match k.code {
            KeyCode::Char('n') | KeyCode::Tab => self.step(1),
            KeyCode::Char('N') | KeyCode::Char('p') | KeyCode::BackTab => self.step(-1),
            KeyCode::Char(' ') => self.toggle(None),
            KeyCode::Right => self.toggle(Some(true)),
            KeyCode::Left => self.toggle(Some(false)),
            KeyCode::Char('w') => self.set_whole(!self.whole),
            KeyCode::Char('d') => self.set_whole(false),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::Home => self.move_by(-(isize::MAX / 2)),
            KeyCode::End => self.move_by(isize::MAX / 2),
            _ => return false,
        }
        true
    }

    /// Everything it says, every hunk open and nothing folded, as lines: a row after its mark
    /// (`+ `, `− `, `~ `, `! `, `  `), its note after a dash.
    #[cfg(test)]
    pub(crate) fn said(&self) -> Vec<Said> {
        let mut out = self.top.clone();
        for hunk in &self.hunks {
            let sum: Vec<&str> = hunk.sum.iter().map(|(_, s)| s.as_str()).collect();
            out.push((Tone::Head, if sum.is_empty() { hunk.head.clone() } else { format!("{}  {}", hunk.head, sum.join(" ")) }));
            out.extend(hunk.rows.iter().map(|c| {
                let tone = match c.kind {
                    '+' => Tone::Added,
                    '-' => Tone::Removed,
                    '!' => Tone::Bad,
                    'a' | '~' | 'w' => Tone::Warn,
                    'i' => Tone::Dim,
                    _ => Tone::Plain,
                };
                (tone, format!("{}{}{}", c.sign(), c.text, if c.note.is_empty() { String::new() } else { format!(" — {}", c.note) }))
            }));
        }
        out.extend(self.foot.iter().cloned());
        out
    }
}

/// Whether each row is a change or within `AROUND` rows of one.
fn near_a_change(rows: &[Code]) -> Vec<bool> {
    (0..rows.len()).map(|i| rows[i.saturating_sub(AROUND)..(i + AROUND + 1).min(rows.len())].iter().any(|c| c.kind != ' ')).collect()
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
    let rows = lines
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
            Code { kind, number: Some(number), text, colours, marked, note: String::new() }
        })
        .collect();
    Some(Hunk::new(head, counted(added, removed), rows, true))
}

/// `+added · −removed`, coloured.
fn counted(added: usize, removed: usize) -> Vec<Said> {
    vec![(Tone::Added, format!("+{added}")), (Tone::Removed, format!("−{removed}"))]
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

/// What a flow touches (a preview's `touches` and `settings`), a hunk a group, a row a thing: what
/// the change does to it, its note after it; open when something in it changes or isn't ready.
pub(crate) fn touched(preview: &Value) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = vec![];
    for l in crate::touches::lines(&preview["touches"], &preview["settings"]) {
        let kind = match l.mark {
            Mark::Head => {
                out.push(Hunk::new(l.text, vec![], vec![], false));
                continue;
            }
            Mark::Same => ' ',
            Mark::New => '+',
            Mark::Approve => 'a',
            Mark::Changed => '~',
            Mark::Gone => '-',
            Mark::Unready => '!',
        };
        let Some(hunk) = out.last_mut() else { continue };
        // The thing it is, coloured as a noun is in code; what it does with it, plain.
        let thing = l.text.find(':').unwrap_or(l.text.len());
        let colours = vec![(0, l.text[..thing].chars().count(), Class::Noun)];
        hunk.rows.push(Code { kind, number: None, colours, text: l.text, marked: vec![], note: l.note });
    }
    for hunk in &mut out {
        let n = |k: char| hunk.rows.iter().filter(|r| r.kind == k).count();
        let said = [(n('a'), "to approve", Tone::Warn), (n('+'), "new", Tone::Added), (n('~'), "changed", Tone::Warn), (n('-'), "no longer", Tone::Removed), (n('!'), "not ready", Tone::Bad), (n(' '), "the same", Tone::Dim)];
        hunk.open = said[..5].iter().any(|(k, _, _)| *k > 0);
        hunk.sum = said.into_iter().filter(|(k, _, _)| *k > 0).map(|(k, what, tone)| (tone, format!("{k} {what}"))).collect();
    }
    out
}

/// What putting the state back does (a restore's `state`): what it says of it, then each value
/// as the code that sets it, the value now taken out and the one put back put in.
pub(crate) fn state(change: &Value) -> Hunk {
    let mut rows: Vec<Code> = vec![];
    for l in crate::restore::state_lines(change).lines() {
        let warns = l.contains("doesn't undo") || l.contains("doesn't put back");
        rows.push(Code::said(if warns { 'w' } else { 'i' }, l.trim_start()));
    }
    let mut values: Vec<(String, &Value)> = change["values"].as_object().into_iter().flatten().map(|(k, v)| (format!("state.{k}"), v)).collect();
    for (name, c) in change["collections"].as_object().into_iter().flatten() {
        values.extend(c["items"].as_object().into_iter().flatten().map(|(key, v)| (format!("{name}[{}]", Value::from(key.as_str())), v)));
    }
    let (mut added, mut removed) = (0, 0);
    for (name, v) in values {
        for (kind, value) in [('-', &v["now"]), ('+', &v["then"])] {
            if value.is_null() {
                continue;
            }
            let text = format!("{name} = {}", agent::show(value));
            let colours = colours(&text).swap_remove(0);
            rows.push(Code { kind, colours, text, ..Code::default() });
            if kind == '+' {
                added += 1;
            } else {
                removed += 1;
            }
        }
    }
    for (name, c) in change["collections"].as_object().into_iter().flatten() {
        if c["more"] == true {
            rows.push(Code::said('i', format!("{name}: and more items")));
        }
    }
    Hunk::new("Its state: now → as it would be put back", counted(added, removed), rows, true)
}

/// Lines said as rows of their own kind: a hunk's rows that aren't code.
pub(crate) fn rows(kind: char, lines: impl IntoIterator<Item = String>) -> Vec<Code> {
    lines.into_iter().map(|l| Code::said(kind, l)).collect()
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
        let added = hunk.rows.iter().find(|c| c.kind == '+').unwrap();
        assert_eq!((added.number, &added.text[..], &added.marked[..]), (Some(10), "a10 = secret.key", &[(6, 16)][..]));
        assert_eq!(hunk.rows.iter().find(|c| c.kind == '-').unwrap().number, Some(10));

        let mut r = Review { hunks: vec![hunk], height: 20, ..Review::default() };
        // Lines 1 to 6 fold; 7 to 12 show, around the change.
        assert_eq!(r.rows()[..3], [Row::Head(0), Row::Fold(0, 0, 6), Row::Code(0, 6)]);
        let at = |r: &Review| r.cursor(&r.rows());
        assert_eq!(at(&r), 0, "on the first head");
        r.step(1);
        assert_eq!(at(&r), 1, "n: the fold");
        r.toggle(None);
        assert_eq!(r.rows()[1], Row::Code(0, 0), "the fold opened, the cursor on its first line");
        r.move_by(3);
        assert_eq!(r.rows()[at(&r)], Row::Code(0, 3), "↓ goes a line at a time");
        r.move_by(-10);
        assert_eq!(at(&r), 0, "↑ stops at the top");
        r.move_by(5);
        r.toggle(None);
        assert_eq!((r.rows(), at(&r)), (vec![Row::Head(0)], 0), "space inside a hunk closes it, the cursor on its head");
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
        let row = &hunks[0].rows[0];
        assert_eq!((row.kind, &row.text[..], &row.note[..], &row.colours[..]), ('a', "host b.example: call", "approved by confirming", &[(0, 14, Class::Noun)][..]));
    }

    #[test]
    fn the_state_put_back_is_the_code_that_sets_it_taken_out_and_put_in() {
        let change = json!({ "from": "run 6", "changed": ["n"], "removed": ["#collection:notes"], "runs_since": 2,
            "values": { "n": { "now": { "type": "Number", "value": 3 }, "then": { "type": "Number", "value": 1 } } },
            "collections": { "notes": { "items": { "k": { "now": { "type": "Text", "value": "hi" }, "then": null } } } } });
        let hunk = state(&change);
        let said: Vec<(char, &str)> = hunk.rows.iter().map(|r| (r.kind, r.text.as_str())).collect();
        assert_eq!(said[2..], [('-', "state.n = 3"), ('+', "state.n = 1"), ('-', "notes[\"k\"] = \"hi\"")]);
        assert_eq!(said[0].0, 'i');
        assert_eq!(hunk.sum, counted(1, 2));
        assert!(hunk.rows[2].colours.iter().any(|c| c.2 == Class::Number), "coloured as code: {:?}", hunk.rows[2].colours);
    }
}
