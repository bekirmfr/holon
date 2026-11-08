//! A flow's text as a tab edits it: lines, a cursor and a selection, the edits a person makes
//! with keys, and what undoes them.

/// Lines of text and a cursor in them, by line and character (both from 0).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Buffer {
    pub(crate) lines: Vec<String>,
    pub(crate) row: usize,
    pub(crate) col: usize,
    /// The first line shown.
    pub(crate) top: usize,
    /// Where a selection started, when there is one: it runs from here to the cursor.
    pub(crate) anchor: Option<(usize, usize)>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    /// The last edit was typing: more typing joins it, so one undo takes back a word, not a key.
    typing: bool,
}

/// The text and cursor before an edit.
#[derive(Clone, Debug, PartialEq)]
struct Snap {
    lines: Vec<String>,
    row: usize,
    col: usize,
}

/// How far a tab indents.
const INDENT: &str = "  ";
/// How many edits undo goes back.
const UNDOS: usize = 500;

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// The byte offset of character `col` in `s` (its end when past it).
fn at(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(i, _)| i).unwrap_or(s.len())
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Buffer {
    pub(crate) fn new(text: &str) -> Buffer {
        let mut lines: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect();
        if lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        Buffer { lines, row: 0, col: 0, top: 0, anchor: None, undo: vec![], redo: vec![], typing: false }
    }

    /// The whole text, ending with a newline.
    pub(crate) fn text(&self) -> String {
        let mut s = self.lines.join("\n");
        s.push('\n');
        s
    }

    fn line(&self) -> &str {
        &self.lines[self.row]
    }

    fn clamp(&mut self) {
        self.row = self.row.min(self.lines.len().saturating_sub(1));
        self.col = self.col.min(chars(self.line()));
    }

    // ── undo ────────────────────────────────────────────────────────────────────────────────

    /// Keep the text as it is before an edit. Typing after typing is one edit.
    fn keep(&mut self, typing: bool) {
        if typing && self.typing {
            return;
        }
        self.undo.push(Snap { lines: self.lines.clone(), row: self.row, col: self.col });
        if self.undo.len() > UNDOS {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.typing = typing;
    }

    fn swap(&mut self, from: fn(&mut Buffer) -> &mut Vec<Snap>, to: fn(&mut Buffer) -> &mut Vec<Snap>) -> bool {
        let Some(s) = from(self).pop() else { return false };
        let now = Snap { lines: std::mem::replace(&mut self.lines, s.lines), row: self.row, col: self.col };
        to(self).push(now);
        (self.row, self.col) = (s.row, s.col);
        self.anchor = None;
        self.typing = false;
        self.clamp();
        true
    }

    pub(crate) fn undo(&mut self) -> bool {
        self.swap(|b| &mut b.undo, |b| &mut b.redo)
    }

    pub(crate) fn redo(&mut self) -> bool {
        self.swap(|b| &mut b.redo, |b| &mut b.undo)
    }

    // ── the selection ───────────────────────────────────────────────────────────────────────

    /// Before a move: with `extend` (shift held), the move selects from where the cursor was;
    /// without, it drops the selection.
    pub(crate) fn mark(&mut self, extend: bool) {
        self.clamp();
        if !extend {
            self.anchor = None;
        } else if self.anchor.is_none() {
            self.anchor = Some((self.row, self.col));
        }
        self.typing = false;
    }

    /// The selection, start before end, if it holds anything.
    pub(crate) fn selection(&self) -> Option<((usize, usize), (usize, usize))> {
        let a = self.anchor?;
        let b = (self.row, self.col.min(chars(&self.lines[self.row.min(self.lines.len() - 1)])));
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        (s != e).then_some((s, e))
    }

    pub(crate) fn selected(&self) -> Option<String> {
        let ((r0, c0), (r1, c1)) = self.selection()?;
        if r0 == r1 {
            let l = &self.lines[r0];
            return Some(l[at(l, c0)..at(l, c1)].to_string());
        }
        let mut out = self.lines[r0][at(&self.lines[r0], c0)..].to_string();
        for l in &self.lines[r0 + 1..r1] {
            out.push('\n');
            out.push_str(l);
        }
        out.push('\n');
        out.push_str(&self.lines[r1][..at(&self.lines[r1], c1)]);
        Some(out)
    }

    /// Take out what is selected, leaving the cursor where it started.
    fn cut_selection(&mut self) -> bool {
        let Some(((r0, c0), (r1, c1))) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let tail = self.lines[r1][at(&self.lines[r1], c1)..].to_string();
        let head = self.lines[r0][..at(&self.lines[r0], c0)].to_string();
        self.lines.drain(r0 + 1..=r1);
        self.lines[r0] = head + &tail;
        (self.row, self.col) = (r0, c0);
        self.anchor = None;
        true
    }

    /// Delete the selection, as cut does: an edit of its own.
    pub(crate) fn delete_selection(&mut self) -> bool {
        if self.selection().is_none() {
            return false;
        }
        self.keep(false);
        self.cut_selection()
    }

    pub(crate) fn select_all(&mut self) {
        self.anchor = Some((0, 0));
        self.row = self.lines.len() - 1;
        self.col = chars(self.line());
    }

    // ── typing ──────────────────────────────────────────────────────────────────────────────

    pub(crate) fn insert(&mut self, c: char) {
        // A space starts an edit, which the word after it joins.
        if c.is_whitespace() {
            self.keep(false);
            self.typing = true;
        } else {
            self.keep(true);
        }
        self.cut_selection();
        self.clamp();
        let i = at(self.line(), self.col);
        self.lines[self.row].insert(i, c);
        self.col += 1;
    }

    /// Text pasted or put in whole: one edit.
    pub(crate) fn insert_str(&mut self, s: &str) {
        self.keep(false);
        self.cut_selection();
        self.put(s);
        self.typing = false;
    }

    /// The whole text replaced (formatted, say): one edit, the cursor kept where it can be.
    pub(crate) fn replace(&mut self, text: &str) {
        self.keep(false);
        self.lines = Buffer::new(text).lines;
        self.anchor = None;
        self.typing = false;
        self.clamp();
    }

    fn put(&mut self, s: &str) {
        for c in s.chars() {
            match c {
                '\n' => self.split(false),
                '\r' => {}
                '\t' => self.put(INDENT),
                c => {
                    self.clamp();
                    let i = at(self.line(), self.col);
                    self.lines[self.row].insert(i, c);
                    self.col += 1;
                }
            }
        }
    }

    /// Indent: the selected lines when lines are selected, else spaces at the cursor.
    pub(crate) fn tab(&mut self) {
        match self.selection() {
            Some(((r0, _), (r1, _))) if r1 > r0 => {
                self.keep(false);
                for l in &mut self.lines[r0..=r1] {
                    l.insert_str(0, INDENT);
                }
                self.anchor = Some((r0, 0));
                self.col += INDENT.len();
            }
            _ => self.insert_str(INDENT),
        }
    }

    /// Take an indent off the cursor's line, or each selected one.
    pub(crate) fn outdent(&mut self) {
        let (r0, r1) = self.selection().map(|((a, _), (b, _))| (a, b)).unwrap_or((self.row, self.row));
        self.keep(false);
        for r in r0..=r1 {
            let n = self.lines[r].chars().take(INDENT.len()).take_while(|c| *c == ' ').count();
            self.lines[r].replace_range(..n, "");
            if r == self.row {
                self.col = self.col.saturating_sub(n);
            }
        }
    }

    /// Split the line at the cursor; with `indent`, the new line starts as indented as this one,
    /// and further after an opening brace.
    pub(crate) fn newline(&mut self, indent: bool) {
        self.keep(false);
        self.cut_selection();
        self.split(indent);
    }

    fn split(&mut self, indent: bool) {
        self.clamp();
        let i = at(self.line(), self.col);
        let rest = self.lines[self.row].split_off(i);
        let mut lead = String::new();
        if indent {
            let line = self.line();
            lead = line.chars().take_while(|c| *c == ' ').collect();
            if line.trim_end().ends_with('{') {
                lead.push_str(INDENT);
            }
        }
        self.col = chars(&lead);
        self.lines.insert(self.row + 1, lead + rest.trim_start_matches(if indent { ' ' } else { '\0' }));
        self.row += 1;
    }

    pub(crate) fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.keep(false);
        self.clamp();
        if self.col > 0 {
            let line = &self.lines[self.row];
            // In leading spaces, a whole indent at once.
            let n = if line.chars().take(self.col).all(|c| c == ' ') && self.col.is_multiple_of(INDENT.len()) { INDENT.len() } else { 1 };
            let (a, b) = (at(line, self.col - n), at(line, self.col));
            self.lines[self.row].replace_range(a..b, "");
            self.col -= n;
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = chars(self.line());
            self.lines[self.row].push_str(&line);
        }
    }

    pub(crate) fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.keep(false);
        self.clamp();
        let n = chars(self.line());
        if self.col < n {
            let line = &self.lines[self.row];
            let (a, b) = (at(line, self.col), at(line, self.col + 1));
            self.lines[self.row].replace_range(a..b, "");
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }

    /// Put `text` in place of the `back` characters before the cursor: a completion taken.
    pub(crate) fn complete(&mut self, back: usize, text: &str) {
        self.keep(false);
        self.anchor = None;
        self.clamp();
        let from = self.col.saturating_sub(back);
        let line = &self.lines[self.row];
        let (a, b) = (at(line, from), at(line, self.col));
        self.lines[self.row].replace_range(a..b, text);
        self.col = from + chars(text);
    }

    /// `//` before each selected line, or the cursor's (a flow is TypeScript); taken off again
    /// when they all have one.
    pub(crate) fn toggle_comment(&mut self) {
        let (r0, r1) = self.selection().map(|((a, _), (b, ca))| (a, if ca == 0 && b > a { b - 1 } else { b })).unwrap_or((self.row, self.row));
        self.keep(false);
        let rows: Vec<usize> = (r0..=r1).filter(|r| !self.lines[*r].trim().is_empty()).collect();
        let commented = !rows.is_empty() && rows.iter().all(|r| self.lines[*r].trim_start().starts_with("//"));
        let indent = rows.iter().map(|r| self.lines[*r].chars().take_while(|c| *c == ' ').count()).min().unwrap_or(0);
        for r in rows {
            let l = &mut self.lines[r];
            if commented {
                let i = l.find("//").unwrap();
                let n = if l[i + 2..].starts_with(' ') { 3 } else { 2 };
                l.replace_range(i..i + n, "");
                if r == self.row {
                    self.col = self.col.saturating_sub(n);
                }
            } else {
                l.insert_str(at(l, indent), "// ");
                if r == self.row {
                    self.col += 3;
                }
            }
        }
    }

    /// Move the cursor's line, or the selected lines, up or down one.
    pub(crate) fn move_lines(&mut self, down: bool) {
        let (r0, r1) = self.selection().map(|((a, _), (b, _))| (a, b)).unwrap_or((self.row, self.row));
        if (down && r1 + 1 >= self.lines.len()) || (!down && r0 == 0) {
            return;
        }
        self.keep(false);
        if down {
            let l = self.lines.remove(r1 + 1);
            self.lines.insert(r0, l);
        } else {
            let l = self.lines.remove(r0 - 1);
            self.lines.insert(r1, l);
        }
        let by: isize = if down { 1 } else { -1 };
        self.row = (self.row as isize + by) as usize;
        if let Some((ar, ac)) = self.anchor {
            self.anchor = Some(((ar as isize + by) as usize, ac));
        }
    }

    pub(crate) fn duplicate_line(&mut self) {
        self.keep(false);
        let l = self.line().to_string();
        self.lines.insert(self.row + 1, l);
        self.row += 1;
        self.anchor = None;
    }

    // ── moving ──────────────────────────────────────────────────────────────────────────────

    pub(crate) fn left(&mut self) {
        self.clamp();
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = chars(self.line());
        }
    }

    pub(crate) fn right(&mut self) {
        self.clamp();
        if self.col < chars(self.line()) {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// To the start of the word before the cursor.
    pub(crate) fn word_left(&mut self) {
        self.clamp();
        if self.col == 0 {
            return self.left();
        }
        let cs: Vec<char> = self.line().chars().collect();
        let mut c = self.col;
        while c > 0 && cs[c - 1].is_whitespace() {
            c -= 1;
        }
        let w = c > 0 && word(cs[c - 1]);
        while c > 0 && !cs[c - 1].is_whitespace() && word(cs[c - 1]) == w {
            c -= 1;
        }
        self.col = c;
    }

    /// To the end of the word after the cursor.
    pub(crate) fn word_right(&mut self) {
        self.clamp();
        let cs: Vec<char> = self.line().chars().collect();
        if self.col >= cs.len() {
            return self.right();
        }
        let mut c = self.col;
        while c < cs.len() && cs[c].is_whitespace() {
            c += 1;
        }
        let w = c < cs.len() && word(cs[c]);
        while c < cs.len() && !cs[c].is_whitespace() && word(cs[c]) == w {
            c += 1;
        }
        self.col = c;
    }

    /// Up or down `n` lines (negative: up), keeping the column where the line is long enough.
    pub(crate) fn down(&mut self, n: isize) {
        self.row = (self.row as isize + n).clamp(0, self.lines.len() as isize - 1) as usize;
    }

    pub(crate) fn home(&mut self) {
        // To the first character, then the line's start.
        let first = self.line().chars().take_while(|c| *c == ' ').count();
        self.col = if self.col == first { 0 } else { first };
    }

    pub(crate) fn end(&mut self) {
        self.col = chars(self.line());
    }

    /// The cursor, 1-based, as the language's positions count.
    pub(crate) fn pos(&self) -> (u32, u32) {
        let col = self.col.min(chars(&self.lines[self.row.min(self.lines.len() - 1)]));
        (self.row as u32 + 1, col as u32 + 1)
    }

    pub(crate) fn go(&mut self, line: u32, col: u32) {
        self.row = (line.max(1) as usize - 1).min(self.lines.len() - 1);
        self.col = col.max(1) as usize - 1;
        self.anchor = None;
        self.clamp();
    }

    /// The word the cursor is in or just after, as (its first character, the text before the
    /// cursor): what a completion replaces.
    pub(crate) fn word_before(&self) -> (usize, String) {
        let cs: Vec<char> = self.lines[self.row].chars().collect();
        let end = self.col.min(cs.len());
        let mut c = end;
        while c > 0 && word(cs[c - 1]) {
            c -= 1;
        }
        (c, cs[c..end].iter().collect())
    }

    /// Select the next place `query` is found after the cursor, from the top again past the end,
    /// ignoring case. False when it is nowhere.
    pub(crate) fn find(&mut self, query: &str, back: bool) -> bool {
        if query.is_empty() {
            return false;
        }
        let q: Vec<char> = query.to_lowercase().chars().collect();
        let hits = |l: &str| -> Vec<usize> {
            let cs: Vec<char> = l.to_lowercase().chars().collect();
            (0..cs.len().saturating_sub(q.len() - 1)).filter(|i| cs[*i..].starts_with(&q)).collect()
        };
        let n = self.lines.len();
        let from = self.selection().map(|(s, _)| s).unwrap_or((self.row, self.col));
        for k in 0..=n {
            let r = if back { (from.0 + n * 2 - k) % n } else { (from.0 + k) % n };
            let mut cols = hits(&self.lines[r]);
            if back {
                cols.reverse();
            }
            let found = cols.into_iter().find(|c| match (k, back) {
                (0, false) => *c > from.1 || (self.selection().is_none() && *c == from.1),
                (0, true) => *c < from.1,
                (k, _) if k == n => if back { *c >= from.1 } else { *c <= from.1 },
                _ => true,
            });
            if let Some(c) = found {
                self.anchor = Some((r, c));
                (self.row, self.col) = (r, c + q.len());
                return true;
            }
        }
        false
    }

    /// Keep the cursor among the `height` lines shown.
    pub(crate) fn scroll(&mut self, height: usize) {
        let height = height.max(1);
        if self.row < self.top {
            self.top = self.row;
        } else if self.row >= self.top + height {
            self.top = self.row + 1 - height;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_splitting_and_joining_lines() {
        let mut b = Buffer::new("flow a\n");
        assert_eq!(b.lines, vec!["flow a"]);
        b.end();
        b.insert_str(" {");
        b.newline(true);
        assert_eq!((b.row, b.col), (1, 2), "a line after a brace starts indented further");
        b.insert_str("x = 1");
        assert_eq!(b.text(), "flow a {\n  x = 1\n");
        b.home();
        b.home();
        assert_eq!(b.col, 0);
        b.backspace();
        assert_eq!(b.text(), "flow a {  x = 1\n", "backspace at a line's start joins it to the one before");
        b.newline(true);
        assert_eq!(b.text(), "flow a {\n  x = 1\n");
        b.down(-1);
        b.end();
        b.delete();
        assert_eq!(b.text(), "flow a {  x = 1\n");
    }

    #[test]
    fn backspace_in_an_indent_takes_the_whole_indent() {
        let mut b = Buffer::new("    é");
        b.go(1, 5);
        b.backspace();
        assert_eq!(b.lines[0], "  é");
        b.end();
        b.backspace();
        assert_eq!(b.lines[0], "  ", "a character, by characters not bytes");
    }

    #[test]
    fn the_cursor_stays_shown() {
        let mut b = Buffer::new(&"x\n".repeat(50));
        b.down(30);
        b.scroll(10);
        assert_eq!(b.top, 21);
        b.down(-25);
        b.scroll(10);
        assert_eq!(b.top, 5);
    }

    #[test]
    fn undo_takes_back_a_word_typed_and_redo_puts_it_again() {
        let mut b = Buffer::new("flow a\n");
        b.end();
        for c in " beats".chars() {
            b.insert(c);
        }
        b.newline(true);
        b.insert('x');
        assert_eq!(b.text(), "flow a beats\nx\n");
        assert!(b.undo());
        assert_eq!(b.text(), "flow a beats\n\n");
        assert!(b.undo());
        assert_eq!(b.text(), "flow a beats\n");
        assert!(b.undo());
        assert_eq!(b.text(), "flow a\n", "the space before a word starts the word's edit");
        assert!(!b.undo());
        assert!(b.redo() && b.redo());
        assert_eq!(b.text(), "flow a beats\n\n");
        b.insert('y');
        assert!(!b.redo(), "an edit after undoing drops what redo had");
    }

    #[test]
    fn a_selection_is_copied_replaced_and_deleted_across_lines() {
        let mut b = Buffer::new("one\ntwo\nthree\n");
        b.go(1, 2);
        b.mark(true);
        b.down(2);
        b.col = 2;
        assert_eq!(b.selected().as_deref(), Some("ne\ntwo\nth"));
        b.insert('X');
        assert_eq!(b.text(), "oXree\n", "typing replaces what is selected");
        b.undo();
        assert_eq!(b.text(), "one\ntwo\nthree\n");
        b.select_all();
        b.backspace();
        assert_eq!(b.text(), "\n");
    }

    #[test]
    fn comments_moves_and_finds() {
        let mut b = Buffer::new("a = 1\n  b = 2\n");
        b.mark(true);
        b.down(1);
        b.end();
        b.toggle_comment();
        assert_eq!(b.text(), "// a = 1\n//   b = 2\n", "as TypeScript comments");
        b.toggle_comment();
        assert_eq!(b.text(), "a = 1\n  b = 2\n", "and back");
        b.mark(false);
        b.go(1, 1);
        b.move_lines(true);
        assert_eq!(b.text(), "  b = 2\na = 1\n");
        assert_eq!(b.row, 1);
        assert!(b.find("B", false));
        assert_eq!((b.row, b.col, b.selected().as_deref()), (0, 3, Some("b")), "from the top again, ignoring case");
        assert!(!b.find("zz", false));
        b.go(2, 6);
        assert_eq!(b.word_before(), (4, "1".to_string()));
        b.word_left();
        assert_eq!(b.col, 4);
        b.complete(0, "10");
        assert_eq!(b.lines[1], "a = 101");
    }
}
