//! A flow's text as a tab edits it: lines, a cursor, and the edits a person makes with keys.

/// Lines of text and a cursor in them, by line and character (both from 0).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Buffer {
    pub(crate) lines: Vec<String>,
    pub(crate) row: usize,
    pub(crate) col: usize,
    /// The first line shown.
    pub(crate) top: usize,
}

/// How far a tab indents.
const INDENT: &str = "  ";

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// The byte offset of character `col` in `s` (its end when past it).
fn at(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(i, _)| i).unwrap_or(s.len())
}

impl Buffer {
    pub(crate) fn new(text: &str) -> Buffer {
        let mut lines: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect();
        if lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        Buffer { lines, row: 0, col: 0, top: 0 }
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

    pub(crate) fn insert(&mut self, c: char) {
        self.clamp();
        let i = at(self.line(), self.col);
        self.lines[self.row].insert(i, c);
        self.col += 1;
    }

    pub(crate) fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            match c {
                '\n' => self.newline(false),
                '\r' => {}
                '\t' => self.insert_str(INDENT),
                c => self.insert(c),
            }
        }
    }

    pub(crate) fn tab(&mut self) {
        self.insert_str(INDENT);
    }

    /// Split the line at the cursor; with `indent`, the new line starts as indented as this one,
    /// and further after an opening brace.
    pub(crate) fn newline(&mut self, indent: bool) {
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
        self.clamp();
        if self.col > 0 {
            let line = &self.lines[self.row];
            // In leading spaces, a whole indent at once.
            let n = if line.chars().take(self.col).all(|c| c == ' ') && self.col % INDENT.len() == 0 { INDENT.len() } else { 1 };
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
        self.clamp();
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
}
