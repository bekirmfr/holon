//! The source formatter: a `.holon.ts` text as `holon fmt` writes it (docs/L4s/typescript-syntax-L4.md
//! §13). It lays the TypeScript out as written, Prettier's way: two spaces, double quotes, no
//! semicolons, 100 columns, a list that doesn't fit broken one item to a line with a trailing
//! comma, an object the text broke kept broken, a function passed last hugged by its call. It
//! keeps every comment and a blank line where the text had one, rewrites `var` to `const` (or
//! `let` where it is assigned again) and `==` to `===`, and never touches a `js()` body, whose text
//! is its identity. One rule is Holon's own: a handler's options after its function stay beside
//! the function's closing brace, `}, { log: "lazy" })`, as Prettier keeps a timeout's delay.
//!
//! The text must compile, and what fmt writes compiles to the same flow: it checks both before it
//! answers. A comment it can't place, one inside an expression, keeps its statement as written.

use std::collections::BTreeSet;

use oxc_allocator::Allocator;
use oxc_ast::ast as js;
use oxc_ast::ast::CommentKind;
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span as At};

use super::ast::{Pos, Span};
use super::lower::LowerError;
use super::ts;

/// The width a line is laid out to.
const WIDTH: usize = 100;

/// `src` laid out as `holon fmt` writes it, or why it can't be: the mistake that keeps it from
/// compiling, said at its place.
pub fn format(src: &str) -> Result<String, LowerError> {
    let src = src.replace("\r\n", "\n");
    let before = ts::compile(&src)?;
    let out = lay_out(&src)?;
    let after = ts::compile(&out).map_err(|e| mistake(format!("fmt wrote a text that doesn't compile ({e}), so it leaves this one as it is: a mistake in fmt")))?;
    if before.name != after.name || unplaced(&before.doc) != unplaced(&after.doc) {
        return Err(mistake("fmt's layout would change what this flow does, so it leaves the text as it is: a mistake in fmt".into()));
    }
    if comments(&src) != comments(&out) {
        return Err(mistake("fmt's layout would lose a comment, so it leaves the text as it is: a mistake in fmt".into()));
    }
    Ok(out)
}

/// The text's comments in order, each as written but for spaces at its lines' ends.
fn comments(src: &str) -> Vec<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, src, SourceType::ts()).parse();
    let trimmed = |c: &str| c.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
    parsed.program.comments.iter().map(|c| trimmed(&src[c.span.start as usize..c.span.end as usize])).collect()
}

/// A flow document without where in the text each node was written, which a layout moves.
fn unplaced(doc: &crate::builder::FlowDocument) -> serde_json::Value {
    fn strip(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                if m.get("source").is_some_and(|s| s.get("line").is_some()) {
                    m.remove("source");
                }
                m.values_mut().for_each(strip);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut v = serde_json::to_value(doc).unwrap_or_default();
    strip(&mut v);
    v
}

fn mistake(message: String) -> LowerError {
    let at = Pos { line: 1, col: 1 };
    LowerError { span: Span { start: at, end: at }, message }
}

/// The layout, with no check that it is the same flow. A statement holding a comment no list
/// placed is laid out again as written, until every comment is placed.
fn lay_out(src: &str) -> Result<String, LowerError> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, src, SourceType::ts()).parse();
    if let Some(e) = parsed.diagnostics.errors().next() {
        return Err(mistake(e.message.to_string()));
    }
    let program = &parsed.program;
    let notes: Vec<Note> = program.comments.iter().map(|c| Note { start: c.span.start, end: c.span.end, line: c.kind == CommentKind::Line }).collect();
    let mut writes = Writes::default();
    writes.visit_program(program);
    let mut kept = BTreeSet::new();
    loop {
        let mut f = Fmt { src, notes: &notes, used: vec![false; notes.len()], kept: &kept, writes: &writes.0, statements: Vec::new() };
        let doc = f.program(program);
        let unplaced: Vec<Note> = notes.iter().zip(&f.used).filter(|(_, used)| !**used).map(|(n, _)| *n).collect();
        if unplaced.is_empty() {
            return Ok(render(&doc));
        }
        let mut more = Vec::new();
        for n in unplaced {
            let around = f.statements.iter().filter(|s| s.start <= n.start && n.end <= s.end && !kept.contains(&s.start)).min_by_key(|s| s.end - s.start);
            match around {
                Some(s) => more.push(s.start),
                None => return Err(mistake(format!("fmt can't place the comment at byte {}", n.start))),
            }
        }
        kept.extend(more);
    }
}

/// A comment, by its place in the text.
#[derive(Clone, Copy, Debug)]
struct Note {
    start: u32,
    end: u32,
    /// A `//` comment, which ends its line.
    line: bool,
}

/// The names the text assigns again (`x = …`, `x++`, `({ x } = …)`): a `var` among them is a `let`.
#[derive(Default)]
struct Writes(BTreeSet<String>);

impl<'a> Visit<'a> for Writes {
    fn visit_simple_assignment_target(&mut self, it: &js::SimpleAssignmentTarget<'a>) {
        if let js::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = it {
            self.0.insert(id.name.to_string());
        }
        oxc_ast_visit::walk::walk_simple_assignment_target(self, it);
    }

    fn visit_assignment_target_property_identifier(&mut self, it: &js::AssignmentTargetPropertyIdentifier<'a>) {
        self.0.insert(it.binding.name.to_string());
        oxc_ast_visit::walk::walk_assignment_target_property_identifier(self, it);
    }
}

/// A layout, before it is fitted to the width: Prettier's document, smaller.
#[derive(Clone, Debug)]
enum Doc {
    Text(String),
    Line(Brk),
    Indent(Box<Doc>),
    Cat(Vec<Doc>),
    /// Laid out on one line if it fits, else broken. With `states`, the first of them that fits
    /// (the first is `doc`), else the last broken: a call that hugs its last argument.
    Group { doc: Box<Doc>, broken: bool, states: Vec<Doc> },
    /// The first when its group is broken, the second when it is on one line.
    IfBreak(Box<Doc>, Box<Doc>),
    /// Written at the end of the line: a comment after a statement.
    Suffix(String),
    /// Breaks the groups around it.
    BreakParent,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Brk {
    /// Nothing on one line, a new line when broken.
    Soft,
    /// A space on one line, a new line when broken.
    Space,
    /// Always a new line.
    Hard,
    /// A new line of text as written, without indenting: inside a template or a `js()` body.
    Literal,
}

fn text(s: impl Into<String>) -> Doc {
    Doc::Text(s.into())
}

fn cat(parts: Vec<Doc>) -> Doc {
    Doc::Cat(parts)
}

fn indent(d: Doc) -> Doc {
    Doc::Indent(Box::new(d))
}

fn soft() -> Doc {
    Doc::Line(Brk::Soft)
}

fn line() -> Doc {
    Doc::Line(Brk::Space)
}

fn hard() -> Doc {
    Doc::Line(Brk::Hard)
}

fn if_break(broken: &str) -> Doc {
    Doc::IfBreak(Box::new(text(broken)), Box::new(text("")))
}

fn group(d: Doc) -> Doc {
    let broken = forced(&d);
    Doc::Group { doc: Box::new(d), broken, states: Vec::new() }
}

fn broken(d: Doc) -> Doc {
    Doc::Group { doc: Box::new(d), broken: true, states: Vec::new() }
}

fn choice(states: Vec<Doc>) -> Doc {
    Doc::Group { doc: Box::new(states[0].clone()), broken: false, states }
}

fn join(items: Vec<Doc>, sep: impl Fn() -> Doc) -> Vec<Doc> {
    let mut out = Vec::new();
    for (i, d) in items.into_iter().enumerate() {
        if i > 0 {
            out.push(sep());
        }
        out.push(d);
    }
    out
}

/// Whether `d` must break the group it is in: it holds a new line that always is one. A group
/// with states stops it, as the state that fits decides.
fn forced(d: &Doc) -> bool {
    match d {
        Doc::Line(Brk::Hard) | Doc::BreakParent => true,
        Doc::Cat(parts) => parts.iter().any(forced),
        Doc::Indent(x) => forced(x),
        Doc::IfBreak(b, f) => forced(b) || forced(f),
        Doc::Group { broken, states, .. } => states.is_empty() && *broken,
        _ => false,
    }
}

/// Whether `d` breaks however wide the line: [`forced`], looking into a group with states too.
fn will_break(d: &Doc) -> bool {
    match d {
        Doc::Line(Brk::Hard) | Doc::BreakParent => true,
        Doc::Cat(parts) => parts.iter().any(will_break),
        Doc::Indent(x) => will_break(x),
        Doc::IfBreak(b, f) => will_break(b) || will_break(f),
        Doc::Group { doc, broken, .. } => *broken || will_break(doc),
        _ => false,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Break,
    Flat,
}

/// The document fitted to the width, as Prettier's printer does.
fn render(doc: &Doc) -> String {
    let mut out = String::new();
    let mut col = 0usize;
    let mut cmds: Vec<(usize, Mode, &Doc)> = vec![(0, Mode::Break, doc)];
    let mut suffix: Vec<&str> = Vec::new();
    // After a new line laid out inside a group on one line, the next group is measured again.
    let mut remeasure = false;
    while let Some((ind, mode, d)) = cmds.pop() {
        match d {
            Doc::Text(s) => {
                out.push_str(s);
                col += s.chars().count();
            }
            Doc::Cat(parts) => cmds.extend(parts.iter().rev().map(|p| (ind, mode, p))),
            Doc::Indent(x) => cmds.push((ind + 2, mode, x)),
            Doc::IfBreak(b, f) => cmds.push((ind, mode, if mode == Mode::Break { b } else { f })),
            Doc::Suffix(s) => suffix.push(s),
            Doc::BreakParent => {}
            Doc::Group { doc: inner, broken, states } => {
                if mode == Mode::Flat && !remeasure {
                    cmds.push((ind, if *broken { Mode::Break } else { Mode::Flat }, inner));
                    continue;
                }
                remeasure = false;
                let rem = WIDTH as isize - col as isize;
                let flat = (ind, Mode::Flat, &**inner);
                if !broken && fits(flat, &cmds, rem, false) {
                    cmds.push(flat);
                } else if let Some(most) = states.last() {
                    if *broken {
                        cmds.push((ind, Mode::Break, most));
                    } else {
                        let pick = states[1..].iter().map(|s| (ind, Mode::Flat, s)).find(|&c| fits(c, &cmds, rem, false));
                        cmds.push(pick.unwrap_or((ind, Mode::Break, most)));
                    }
                } else {
                    cmds.push((ind, Mode::Break, inner));
                }
            }
            Doc::Line(b) => match (mode, b) {
                (Mode::Flat, Brk::Soft) => {}
                (Mode::Flat, Brk::Space) => {
                    out.push(' ');
                    col += 1;
                }
                _ => {
                    for s in suffix.drain(..) {
                        out.push_str(s);
                    }
                    if *b == Brk::Literal {
                        out.push('\n');
                        col = 0;
                    } else {
                        let kept = out.trim_end_matches([' ', '\t']).len();
                        out.truncate(kept);
                        out.push('\n');
                        out.push_str(&" ".repeat(ind));
                        col = ind;
                    }
                    if mode == Mode::Flat {
                        remeasure = true;
                    }
                }
            },
        }
    }
    for s in suffix {
        out.push_str(s);
    }
    let kept = out.trim_end().len();
    out.truncate(kept);
    out.push('\n');
    out
}

/// Whether `next` fits in `width` columns, up to the first new line it or what follows it breaks.
fn fits(next: (usize, Mode, &Doc), rest: &[(usize, Mode, &Doc)], mut width: isize, must_be_flat: bool) -> bool {
    let mut rest_at = rest.len();
    let mut cmds: Vec<(Mode, &Doc)> = vec![(next.1, next.2)];
    while width >= 0 {
        let (mode, d) = match cmds.pop() {
            Some(c) => c,
            None if rest_at == 0 => return true,
            None => {
                rest_at -= 1;
                (rest[rest_at].1, rest[rest_at].2)
            }
        };
        match d {
            Doc::Text(s) => width -= s.chars().count() as isize,
            Doc::Cat(parts) => cmds.extend(parts.iter().rev().map(|p| (mode, p))),
            Doc::Indent(x) => cmds.push((mode, x)),
            Doc::IfBreak(b, f) => cmds.push((mode, if mode == Mode::Break { b } else { f })),
            Doc::Group { doc, broken, states } => {
                if must_be_flat && *broken {
                    return false;
                }
                let m = if *broken { Mode::Break } else { mode };
                cmds.push((m, if m == Mode::Break && !states.is_empty() { states.last().expect("some") } else { doc }));
            }
            Doc::Line(b) => {
                if mode == Mode::Break || matches!(b, Brk::Hard | Brk::Literal) {
                    return true;
                }
                if *b == Brk::Space {
                    width -= 1;
                }
            }
            Doc::Suffix(_) | Doc::BreakParent => {}
        }
    }
    false
}

/// How tightly an operator binds, JavaScript's order.
fn precedence(op: &str) -> u8 {
    match op {
        "??" => 1,
        "||" => 2,
        "&&" => 3,
        "|" => 4,
        "^" => 5,
        "&" => 6,
        "==" | "!=" | "===" | "!==" => 7,
        "<" | ">" | "<=" | ">=" | "in" | "instanceof" => 8,
        "<<" | ">>" | ">>>" => 9,
        "+" | "-" => 10,
        "*" | "/" | "%" => 11,
        _ => 12,
    }
}

/// Whether `child`, the left of `parent`, is laid out as links of one chain with it: the same
/// precedence, but for equality, shifts, `**`, and `*`, `/` and `%` mixed, which Prettier keeps
/// apart.
fn flattens(parent: &str, child: &str) -> bool {
    let p = precedence(parent);
    p == precedence(child) && !matches!(p, 7 | 9 | 12) && (p != 11 || parent == child)
}

/// What an argument is, for hugging.
fn is_function(e: &js::Expression) -> bool {
    matches!(e, js::Expression::ArrowFunctionExpression(_) | js::Expression::FunctionExpression(_))
}

fn block_bodied(e: &js::Expression) -> bool {
    match e {
        js::Expression::ArrowFunctionExpression(a) => matches!(a.body, js::ArrowFunctionBody::FunctionBody(_)),
        js::Expression::FunctionExpression(_) => true,
        _ => false,
    }
}

/// Whether a statement that begins with `e` needs a `;` before it, written without semicolons.
fn risky(e: &js::Expression) -> bool {
    use js::Expression as E;
    match e {
        E::ParenthesizedExpression(_) | E::ArrayExpression(_) | E::TemplateLiteral(_) | E::RegExpLiteral(_) => true,
        E::UnaryExpression(u) => matches!(u.operator.as_str(), "-" | "+"),
        E::UpdateExpression(u) => u.prefix,
        E::CallExpression(c) => risky(&c.callee),
        E::StaticMemberExpression(m) => risky(&m.object),
        E::ComputedMemberExpression(m) => risky(&m.object),
        E::TaggedTemplateExpression(t) => risky(&t.tag),
        E::BinaryExpression(b) => risky(&b.left),
        E::LogicalExpression(l) => risky(&l.left),
        E::ConditionalExpression(c) => risky(&c.test),
        E::SequenceExpression(s) => s.expressions.first().is_some_and(risky),
        E::TSAsExpression(a) => risky(&a.expression),
        E::TSSatisfiesExpression(a) => risky(&a.expression),
        E::TSNonNullExpression(a) => risky(&a.expression),
        E::AssignmentExpression(a) => match &a.left {
            js::AssignmentTarget::ArrayAssignmentTarget(_) => true,
            js::AssignmentTarget::StaticMemberExpression(m) => risky(&m.object),
            js::AssignmentTarget::ComputedMemberExpression(m) => risky(&m.object),
            _ => false,
        },
        E::ChainExpression(c) => match &c.expression {
            js::ChainElement::CallExpression(x) => risky(&x.callee),
            js::ChainElement::StaticMemberExpression(m) => risky(&m.object),
            js::ChainElement::ComputedMemberExpression(m) => risky(&m.object),
            _ => false,
        },
        _ => false,
    }
}

/// The names a binding pattern binds.
fn bound(p: &js::BindingPattern, out: &mut Vec<String>) {
    match p {
        js::BindingPattern::BindingIdentifier(id) => out.push(id.name.to_string()),
        js::BindingPattern::ObjectPattern(o) => {
            o.properties.iter().for_each(|p| bound(&p.value, out));
            if let Some(r) = &o.rest {
                bound(&r.argument, out);
            }
        }
        js::BindingPattern::ArrayPattern(a) => {
            a.elements.iter().flatten().for_each(|p| bound(p, out));
            if let Some(r) = &a.rest {
                bound(&r.argument, out);
            }
        }
        js::BindingPattern::AssignmentPattern(a) => bound(&a.left, out),
    }
}

struct Fmt<'s> {
    src: &'s str,
    notes: &'s [Note],
    used: Vec<bool>,
    /// The statements laid out as written, by where they start: each holds a comment no list placed.
    kept: &'s BTreeSet<u32>,
    writes: &'s BTreeSet<String>,
    /// Every statement laid out, for finding the one around a comment.
    statements: Vec<At>,
}

impl<'s> Fmt<'s> {
    fn at(&self, s: At) -> &'s str {
        &self.src[s.start as usize..s.end as usize]
    }

    /// Text as written, its lines as they are, the first where it lands.
    fn verbatim(&mut self, s: At) -> Doc {
        for (i, n) in self.notes.iter().enumerate() {
            if s.start <= n.start && n.end <= s.end {
                self.used[i] = true;
            }
        }
        let mut parts = Vec::new();
        for (i, l) in self.at(s).split('\n').enumerate() {
            if i > 0 {
                parts.push(Doc::Line(Brk::Literal));
            }
            parts.push(text(l));
        }
        cat(parts)
    }

    fn note(&mut self, n: Note) -> Doc {
        self.verbatim(At::new(n.start, n.end))
    }

    /// The comments from `from` up to `to` no list has placed, placed now.
    fn notes_in(&mut self, from: u32, to: u32) -> Vec<Note> {
        let mut out = Vec::new();
        for (i, n) in self.notes.iter().enumerate() {
            if !self.used[i] && from <= n.start && n.end <= to {
                self.used[i] = true;
                out.push(*n);
            }
        }
        out
    }

    fn has_notes(&self, from: u32, to: u32) -> bool {
        self.notes.iter().zip(&self.used).any(|(n, used)| !used && from <= n.start && n.end <= to)
    }

    /// The comments on the line `after` ends, before `till`: a statement's or an item's own.
    fn trailing(&mut self, after: u32, till: u32) -> Vec<Note> {
        let mut out = Vec::new();
        let mut at = after;
        for (i, n) in self.notes.iter().enumerate() {
            if self.used[i] || n.start < at {
                continue;
            }
            if n.end > till || self.src[at as usize..n.start as usize].contains('\n') {
                break;
            }
            self.used[i] = true;
            out.push(*n);
            at = n.end;
        }
        out
    }

    /// Whether the text between `a` and `b` holds an empty line.
    fn blank(&self, a: u32, b: u32) -> bool {
        let gap: Vec<&str> = self.src[a as usize..b as usize].split('\n').collect();
        gap.len() > 2 && gap[1..gap.len() - 1].iter().any(|l| l.trim().is_empty())
    }

    /// Where `ch` is next, from `from` on.
    fn find(&self, from: u32, ch: char) -> u32 {
        from + self.src[from as usize..].find(ch).unwrap_or(0) as u32
    }

    /// A string as written, in double quotes where single ones need no escape to become them.
    fn string(&self, s: At) -> String {
        let raw = self.at(s);
        if !raw.starts_with('\'') || raw[1..raw.len() - 1].contains('"') {
            return raw.to_string();
        }
        let mut out = String::from("\"");
        let mut chars = raw[1..raw.len() - 1].chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('\'') => out.push('\''),
                Some(n) => {
                    out.push('\\');
                    out.push(n);
                }
                None => out.push('\\'),
            }
        }
        out.push('"');
        out
    }

    fn program(&mut self, p: &js::Program) -> Doc {
        let mut parts = Vec::new();
        let mut start = 0;
        if let Some(h) = &p.hashbang {
            parts.push(self.verbatim(h.span));
            parts.push(hard());
            start = h.span.end;
        }
        for d in &p.directives {
            parts.push(self.verbatim(d.span));
            parts.push(hard());
            start = d.span.end;
        }
        parts.push(self.statements(&p.body, start, self.src.len() as u32));
        cat(parts)
    }

    /// Statements one to a line, each with the comments before it and on its line, and a blank
    /// line where the text had one between two of them.
    fn statements(&mut self, list: &[js::Statement], start: u32, end: u32) -> Doc {
        let items: Vec<&js::Statement> = list.iter().filter(|s| !matches!(s, js::Statement::EmptyStatement(_))).collect();
        let mut parts = Vec::new();
        let mut last: Option<u32> = None;
        let mut from = start;
        for (i, s) in items.iter().enumerate() {
            let at = s.span();
            let mut glue = false;
            for n in self.notes_in(from, at.start) {
                self.sep(&mut parts, last, n.start, glue);
                parts.push(self.note(n));
                glue = !n.line && !self.src[n.end as usize..at.start as usize].contains('\n');
                last = Some(n.end);
            }
            self.sep(&mut parts, last, at.start, glue);
            parts.push(self.statement(s));
            let till = items.get(i + 1).map_or(end, |n| n.span().start);
            let mut done = at.end;
            for n in self.trailing(at.end, till) {
                parts.push(Doc::Suffix(format!(" {}", self.at(At::new(n.start, n.end)))));
                parts.push(Doc::BreakParent);
                self.verbatim(At::new(n.start, n.end));
                done = n.end;
            }
            last = Some(done);
            from = done;
        }
        for n in self.notes_in(from, end) {
            self.sep(&mut parts, last, n.start, false);
            parts.push(self.note(n));
            last = Some(n.end);
        }
        cat(parts)
    }

    /// What comes between two pieces of a statement list: a new line, and an empty one where the
    /// text had one; or a space, after a `/* … */` on the piece's own line.
    fn sep(&self, parts: &mut Vec<Doc>, last: Option<u32>, at: u32, glue: bool) {
        let Some(l) = last else { return };
        if glue {
            parts.push(text(" "));
            return;
        }
        parts.push(hard());
        if self.blank(l, at) {
            parts.push(hard());
        }
    }

    fn statement(&mut self, s: &js::Statement) -> Doc {
        let at = s.span();
        self.statements.push(at);
        if self.kept.contains(&at.start) {
            return self.verbatim(at);
        }
        use js::Statement as S;
        match s {
            S::ExpressionStatement(e) => {
                let d = self.expr(&e.expression);
                if risky(&e.expression) {
                    cat(vec![text(";"), d])
                } else {
                    d
                }
            }
            S::VariableDeclaration(v) => self.declaration(v),
            S::FunctionDeclaration(f) => self.function(f),
            S::ReturnStatement(r) => match &r.argument {
                None => text("return"),
                Some(a) => cat(vec![text("return "), self.returned(a)]),
            },
            S::ThrowStatement(t) => cat(vec![text("throw "), self.expr(&t.argument)]),
            S::IfStatement(i) => self.if_statement(i),
            S::BlockStatement(b) => self.block(&b.body, b.span),
            S::SwitchStatement(w) => self.switch(w),
            S::WhileStatement(w) => {
                let test = self.test(&w.test);
                let body = self.clause(&w.body);
                group(cat(vec![text("while ("), test, text(")"), body]))
            }
            S::DoWhileStatement(w) => {
                let body = self.clause(&w.body);
                let after = if matches!(w.body, js::Statement::BlockStatement(_)) { text(" ") } else { hard() };
                cat(vec![group(cat(vec![text("do"), body])), after, text("while ("), self.test(&w.test), text(")")])
            }
            S::BreakStatement(b) if b.label.is_none() => text("break"),
            S::ContinueStatement(c) if c.label.is_none() => text("continue"),
            S::ImportDeclaration(i) => self.import(i),
            S::ExportDeclaration(e) => match &e.declaration {
                js::Declaration::VariableDeclaration(v) => cat(vec![text("export "), self.declaration(v)]),
                _ => self.verbatim(at),
            },
            _ => self.verbatim(at),
        }
    }

    /// A loop's or an `if`'s body: a block, or the one statement.
    fn body(&mut self, s: &js::Statement) -> Doc {
        match s {
            js::Statement::BlockStatement(b) => {
                self.statements.push(b.span);
                if self.kept.contains(&b.span.start) {
                    return self.verbatim(b.span);
                }
                self.block(&b.body, b.span)
            }
            other => self.statement(other),
        }
    }

    fn block(&mut self, body: &[js::Statement], span: At) -> Doc {
        let (start, end) = (span.start + 1, span.end - 1);
        if body.iter().all(|s| matches!(s, js::Statement::EmptyStatement(_))) && !self.has_notes(start, end) {
            return text("{}");
        }
        let inner = self.statements(body, start, end);
        cat(vec![text("{"), indent(cat(vec![hard(), inner])), hard(), text("}")])
    }

    /// The test of an `if` or a loop: an operator chain that doesn't fit breaks inside the
    /// parentheses.
    fn test(&mut self, e: &js::Expression) -> Doc {
        match e {
            js::Expression::BinaryExpression(_) | js::Expression::LogicalExpression(_) => group(cat(vec![indent(cat(vec![soft(), self.binary(e, false)])), soft()])),
            _ => self.expr(e),
        }
    }

    /// What follows an `if (…)`, a loop's head or an `else`: a block after a space, another `if`
    /// after a space, else the statement on the line if it fits, or indented on the next.
    fn clause(&mut self, s: &js::Statement) -> Doc {
        match s {
            js::Statement::BlockStatement(_) | js::Statement::IfStatement(_) => cat(vec![text(" "), self.body(s)]),
            _ => indent(cat(vec![line(), self.statement(s)])),
        }
    }

    fn if_statement(&mut self, i: &js::IfStatement) -> Doc {
        let test = self.test(&i.test);
        let consequent = self.clause(&i.consequent);
        let mut parts = vec![group(cat(vec![text("if ("), test, text(")"), consequent]))];
        if let Some(alt) = &i.alternate {
            let block = matches!(i.consequent, js::Statement::BlockStatement(_));
            parts.push(if block { text(" else") } else { cat(vec![hard(), text("else")]) });
            let alternate = self.clause(alt);
            parts.push(group(alternate));
        }
        cat(parts)
    }

    fn switch(&mut self, w: &js::SwitchStatement) -> Doc {
        let head = cat(vec![text("switch ("), self.expr(&w.discriminant), text(") {")]);
        let open = self.find(w.discriminant.span().end, '{') + 1;
        let end = w.span.end - 1;
        if w.cases.is_empty() && !self.has_notes(open, end) {
            return cat(vec![head, text("}")]);
        }
        let mut parts = Vec::new();
        let mut last: Option<u32> = None;
        let mut from = open;
        for (i, c) in w.cases.iter().enumerate() {
            for n in self.notes_in(from, c.span.start) {
                self.sep(&mut parts, last, n.start, false);
                parts.push(self.note(n));
                last = Some(n.end);
            }
            self.sep(&mut parts, last, c.span.start, false);
            let (label, after) = match &c.test {
                Some(t) => (cat(vec![text("case "), self.expr(t), text(":")]), t.span().end),
                None => (text("default:"), c.span.start),
            };
            let colon = self.find(after, ':') + 1;
            parts.push(match c.consequent.as_slice() {
                [] => label,
                [js::Statement::BlockStatement(_)] => cat(vec![label, text(" "), self.body(&c.consequent[0])]),
                list => cat(vec![label, indent(cat(vec![hard(), self.statements(list, colon, c.span.end)]))]),
            });
            let till = w.cases.get(i + 1).map_or(end, |n| n.span.start);
            let mut done = c.span.end;
            for n in self.trailing(c.span.end, till) {
                parts.push(Doc::Suffix(format!(" {}", self.at(At::new(n.start, n.end)))));
                parts.push(Doc::BreakParent);
                done = n.end;
            }
            last = Some(done);
            from = done;
        }
        for n in self.notes_in(from, end) {
            self.sep(&mut parts, last, n.start, false);
            parts.push(self.note(n));
            last = Some(n.end);
        }
        cat(vec![head, indent(cat(vec![hard(), cat(parts)])), hard(), text("}")])
    }

    fn declaration(&mut self, v: &js::VariableDeclaration) -> Doc {
        let kind = match v.kind {
            js::VariableDeclarationKind::Const => "const",
            js::VariableDeclarationKind::Let => "let",
            js::VariableDeclarationKind::Var => {
                let mut names = Vec::new();
                v.declarations.iter().for_each(|d| bound(&d.id, &mut names));
                let assigned = names.iter().any(|n| self.writes.contains(n));
                if assigned || v.declarations.iter().any(|d| d.init.is_none()) {
                    "let"
                } else {
                    "const"
                }
            }
            _ => return self.verbatim(v.span),
        };
        let mut decls: Vec<Doc> = v.declarations.iter().map(|d| self.declarator(d)).collect();
        if decls.len() == 1 {
            return cat(vec![text(kind), text(" "), decls.remove(0)]);
        }
        let first = decls.remove(0);
        let rest: Vec<Doc> = decls.into_iter().flat_map(|d| [text(","), line(), d]).collect();
        group(cat(vec![text(kind), text(" "), first, indent(cat(rest))]))
    }

    fn declarator(&mut self, d: &js::VariableDeclarator) -> Doc {
        let mut parts = vec![self.pattern(&d.id)];
        if d.definite {
            parts.push(text("!"));
        }
        if let Some(t) = &d.type_annotation {
            parts.push(text(": "));
            parts.push(self.ty(&t.type_annotation));
        }
        if let Some(init) = &d.init {
            parts.push(text(" ="));
            parts.push(self.assigned(init));
        }
        cat(parts)
    }

    /// What follows an `=`: on its line, or an operator chain on the next line when it doesn't
    /// fit. A choice keeps its test on the line, `? …` and `: …` below it.
    fn assigned(&mut self, e: &js::Expression) -> Doc {
        match e {
            js::Expression::BinaryExpression(_) | js::Expression::LogicalExpression(_) => group(indent(cat(vec![line(), self.binary(e, false)]))),
            _ => cat(vec![text(" "), self.expr(e)]),
        }
    }

    /// What a `return` answers: an operator chain that doesn't fit goes in parentheses, broken
    /// inside them, and the parentheses go when it fits.
    fn returned(&mut self, e: &js::Expression) -> Doc {
        let inner = match e {
            js::Expression::ParenthesizedExpression(p) if matches!(p.expression, js::Expression::BinaryExpression(_) | js::Expression::LogicalExpression(_)) => &p.expression,
            other => other,
        };
        match inner {
            js::Expression::BinaryExpression(_) | js::Expression::LogicalExpression(_) => {
                group(cat(vec![if_break("("), indent(cat(vec![soft(), self.binary(inner, false)])), soft(), if_break(")")]))
            }
            _ => self.expr(e),
        }
    }

    fn function(&mut self, f: &js::Function) -> Doc {
        let (Some(id), Some(body)) = (&f.id, &f.body) else { return self.verbatim(f.span) };
        if f.generator || f.type_parameters.is_some() || f.this_param.is_some() {
            return self.verbatim(f.span);
        }
        let mut parts = Vec::new();
        if f.r#async {
            parts.push(text("async "));
        }
        parts.push(text(format!("function {}", id.name)));
        parts.push(self.params(&f.params));
        if let Some(t) = &f.return_type {
            parts.push(text(": "));
            parts.push(self.ty(&t.type_annotation));
        }
        parts.push(text(" "));
        parts.push(self.block(&body.statements, body.span));
        cat(parts)
    }

    fn import(&mut self, i: &js::ImportDeclaration) -> Doc {
        if i.import_kind.is_type() || i.phase.is_some() {
            return self.verbatim(i.span);
        }
        let mut tail = vec![text(self.string(i.source.span))];
        if let Some(w) = &i.with_clause {
            let keyword = if w.keyword == js::WithClauseKeyword::Assert { "assert" } else { "with" };
            let entries: Vec<String> = w.with_entries.iter().map(|e| format!("{}: {}", self.at(e.key.span()), self.string(e.value.span))).collect();
            tail.push(text(format!(" {keyword} {{ {} }}", entries.join(", "))));
        }
        let Some(specs) = &i.specifiers else { return cat(vec![text("import "), cat(tail)]) };
        let mut head = Vec::new();
        let mut named = Vec::new();
        for s in specs {
            match s {
                js::ImportDeclarationSpecifier::ImportDefaultSpecifier(d) => head.push(d.local.name.to_string()),
                js::ImportDeclarationSpecifier::ImportNamespaceSpecifier(n) => head.push(format!("* as {}", n.local.name)),
                js::ImportDeclarationSpecifier::ImportSpecifier(s) => {
                    let imported = self.at(s.imported.span());
                    named.push(if imported == s.local.name.as_str() { text(imported) } else { text(format!("{imported} as {}", s.local.name)) });
                }
            }
        }
        let mut parts = vec![text("import ")];
        let braces = !named.is_empty() || head.is_empty();
        parts.push(text(head.join(", ")));
        if braces {
            if !head.is_empty() {
                parts.push(text(", "));
            }
            if named.is_empty() {
                parts.push(text("{}"));
            } else {
                parts.push(group(cat(vec![text("{"), indent(cat(vec![line(), cat(join(named, || cat(vec![text(","), line()])))])), if_break(","), line(), text("}")])));
            }
        }
        parts.push(text(" from "));
        parts.extend(tail);
        cat(parts)
    }

    fn ty(&mut self, t: &js::TSType) -> Doc {
        self.verbatim(t.span())
    }

    fn params(&mut self, p: &js::FormalParameters) -> Doc {
        let mut items: Vec<Doc> = p.items.iter().map(|x| self.param(x)).collect();
        if let Some(r) = &p.rest {
            let mut rest = vec![text("..."), self.pattern(&r.rest.argument)];
            if let Some(t) = &r.type_annotation {
                rest.push(text(": "));
                rest.push(self.ty(&t.type_annotation));
            }
            items.push(cat(rest));
        }
        if items.is_empty() {
            return text("()");
        }
        let comma = if p.rest.is_none() { if_break(",") } else { text("") };
        group(cat(vec![text("("), indent(cat(vec![soft(), cat(join(items, || cat(vec![text(","), line()])))])), comma, soft(), text(")")]))
    }

    fn param(&mut self, x: &js::FormalParameter) -> Doc {
        if !x.decorators.is_empty() || x.accessibility.is_some() || x.readonly || x.r#override {
            return self.verbatim(x.span);
        }
        let mut parts = vec![self.pattern(&x.pattern)];
        if x.optional {
            parts.push(text("?"));
        }
        if let Some(t) = &x.type_annotation {
            parts.push(text(": "));
            parts.push(self.ty(&t.type_annotation));
        }
        if let Some(i) = &x.initializer {
            parts.push(text(" = "));
            parts.push(self.expr(i));
        }
        cat(parts)
    }

    fn pattern(&mut self, p: &js::BindingPattern) -> Doc {
        match p {
            js::BindingPattern::BindingIdentifier(id) => text(id.name.as_str()),
            js::BindingPattern::AssignmentPattern(a) => cat(vec![self.pattern(&a.left), text(" = "), self.expr(&a.right)]),
            js::BindingPattern::ObjectPattern(o) => {
                let mut items = Vec::new();
                for prop in &o.properties {
                    items.push(if prop.shorthand {
                        self.pattern(&prop.value)
                    } else {
                        cat(vec![self.key(&prop.key, prop.computed), text(": "), self.pattern(&prop.value)])
                    });
                }
                if let Some(r) = &o.rest {
                    items.push(cat(vec![text("..."), self.pattern(&r.argument)]));
                }
                if items.is_empty() {
                    return text("{}");
                }
                let comma = if o.rest.is_none() { if_break(",") } else { text("") };
                group(cat(vec![text("{"), indent(cat(vec![line(), cat(join(items, || cat(vec![text(","), line()])))])), comma, line(), text("}")]))
            }
            js::BindingPattern::ArrayPattern(a) => {
                if a.elements.iter().any(|e| e.is_none()) {
                    return self.verbatim(a.span);
                }
                let mut items: Vec<Doc> = a.elements.iter().flatten().map(|e| self.pattern(e)).collect();
                if let Some(r) = &a.rest {
                    items.push(cat(vec![text("..."), self.pattern(&r.argument)]));
                }
                group(cat(vec![text("["), indent(cat(vec![soft(), cat(join(items, || cat(vec![text(","), line()])))])), soft(), text("]")]))
            }
        }
    }

    fn key(&mut self, k: &js::PropertyKey, computed: bool) -> Doc {
        if computed {
            return cat(vec![text("["), self.verbatim(k.span()), text("]")]);
        }
        match k {
            js::PropertyKey::StringLiteral(s) => text(self.string(s.span)),
            other => text(self.at(other.span())),
        }
    }

    fn expr(&mut self, e: &js::Expression) -> Doc {
        use js::Expression as E;
        match e {
            E::StringLiteral(s) => text(self.string(s.span)),
            E::Identifier(_) | E::NumericLiteral(_) | E::BooleanLiteral(_) | E::NullLiteral(_) | E::BigIntLiteral(_) | E::ThisExpression(_) | E::RegExpLiteral(_) | E::Super(_) => {
                text(self.at(e.span()))
            }
            E::ParenthesizedExpression(p) => cat(vec![text("("), self.expr(&p.expression), text(")")]),
            E::ArrayExpression(a) => self.array(a),
            E::ObjectExpression(o) => self.object(o),
            E::ArrowFunctionExpression(a) => self.arrow(a, false),
            E::CallExpression(c) => self.call(c),
            E::NewExpression(n) if n.type_arguments.is_none() => cat(vec![text("new "), self.expr(&n.callee), self.args(&n.arguments)]),
            E::StaticMemberExpression(m) => self.static_member(m),
            E::ComputedMemberExpression(m) => self.computed_member(m),
            E::ChainExpression(c) => match &c.expression {
                js::ChainElement::CallExpression(x) => self.call(x),
                js::ChainElement::StaticMemberExpression(m) => self.static_member(m),
                js::ChainElement::ComputedMemberExpression(m) => self.computed_member(m),
                js::ChainElement::TSNonNullExpression(n) => cat(vec![self.expr(&n.expression), text("!")]),
                other => self.verbatim(other.span()),
            },
            E::BinaryExpression(_) | E::LogicalExpression(_) => self.binary(e, true),
            E::ConditionalExpression(c) => {
                let test = self.expr(&c.test);
                let yes = self.expr(&c.consequent);
                let no = self.expr(&c.alternate);
                group(cat(vec![test, indent(cat(vec![line(), text("? "), yes, line(), text(": "), no]))]))
            }
            E::AssignmentExpression(a) => {
                let target = self.target(&a.left);
                cat(vec![target, text(format!(" {}", a.operator.as_str())), self.assigned(&a.right)])
            }
            E::UnaryExpression(u) => {
                let op = u.operator.as_str();
                let word = op.chars().all(|c| c.is_ascii_alphabetic());
                let doubled = match &u.argument {
                    E::UnaryExpression(inner) => matches!(op, "-" | "+") && inner.operator.as_str().starts_with(op),
                    E::UpdateExpression(inner) => inner.prefix && matches!(op, "-" | "+") && inner.operator.as_str().starts_with(op),
                    _ => false,
                };
                cat(vec![text(op), text(if word || doubled { " " } else { "" }), self.expr(&u.argument)])
            }
            E::UpdateExpression(u) => {
                let target = text(self.at(u.argument.span()));
                if u.prefix {
                    cat(vec![text(u.operator.as_str()), target])
                } else {
                    cat(vec![target, text(u.operator.as_str())])
                }
            }
            E::AwaitExpression(a) => cat(vec![text("await "), self.expr(&a.argument)]),
            E::TSAsExpression(a) => cat(vec![self.expr(&a.expression), text(" as "), self.ty(&a.type_annotation)]),
            E::TSSatisfiesExpression(a) => cat(vec![self.expr(&a.expression), text(" satisfies "), self.ty(&a.type_annotation)]),
            E::TSNonNullExpression(a) => cat(vec![self.expr(&a.expression), text("!")]),
            // A template, a `js()` function and what Holon refuses are kept as written.
            _ => self.verbatim(e.span()),
        }
    }

    fn static_member(&mut self, m: &js::StaticMemberExpression) -> Doc {
        cat(vec![self.expr(&m.object), text(if m.optional { "?." } else { "." }), text(m.property.name.as_str())])
    }

    fn computed_member(&mut self, m: &js::ComputedMemberExpression) -> Doc {
        cat(vec![self.expr(&m.object), text(if m.optional { "?.[" } else { "[" }), self.expr(&m.expression), text("]")])
    }

    fn target(&mut self, t: &js::AssignmentTarget) -> Doc {
        match t {
            js::AssignmentTarget::AssignmentTargetIdentifier(id) => text(id.name.as_str()),
            js::AssignmentTarget::StaticMemberExpression(m) => self.static_member(m),
            js::AssignmentTarget::ComputedMemberExpression(m) => self.computed_member(m),
            other => self.verbatim(other.span()),
        }
    }

    /// An operator chain, `a + b + c`: on one line if it fits, else broken after each operator,
    /// indented unless what holds it indents it (an `=`, a `return`, an arrow's body).
    fn binary(&mut self, e: &js::Expression, indented: bool) -> Doc {
        // The chain is the operators of the top one's precedence, as Prettier flattens it: `a === b
        // || c` is two links, `a === b` one of them.
        let operator = |e: &js::Expression| match e {
            js::Expression::BinaryExpression(b) => Some(match b.operator.as_str() {
                "==" => "===",
                "!=" => "!==",
                op => op,
            }),
            js::Expression::LogicalExpression(l) => Some(l.operator.as_str()),
            _ => None,
        };
        let top = operator(e).expect("an operator chain");
        let mut rights: Vec<(&str, &js::Expression)> = Vec::new();
        let mut left = e;
        while let Some(op) = operator(left).filter(|op| std::ptr::eq(left, e) || flattens(top, op)) {
            let right = match left {
                js::Expression::BinaryExpression(b) => (&b.left, &b.right),
                js::Expression::LogicalExpression(l) => (&l.left, &l.right),
                _ => unreachable!("an operator"),
            };
            rights.push((op, right.1));
            left = right.0;
        }
        let first = self.expr(left);
        let mut rest = Vec::new();
        for (op, r) in rights.into_iter().rev() {
            rest.push(text(format!(" {op}")));
            rest.push(line());
            rest.push(self.expr(r));
        }
        if indented {
            group(cat(vec![first, indent(cat(rest))]))
        } else {
            group(cat(vec![first, cat(rest)]))
        }
    }

    /// `(req) => …`. Passed last, an arrow whose body breaks onto its own line puts the call's
    /// closing parenthesis on the next one.
    fn arrow(&mut self, a: &js::ArrowFunctionExpression, last: bool) -> Doc {
        if a.type_parameters.is_some() {
            return self.verbatim(a.span);
        }
        let mut head = Vec::new();
        if a.r#async {
            head.push(text("async "));
        }
        head.push(self.params(&a.params));
        if let Some(t) = &a.return_type {
            head.push(text(": "));
            head.push(self.ty(&t.type_annotation));
        }
        head.push(text(" =>"));
        let body = match &a.body {
            js::ArrowFunctionBody::FunctionBody(b) => {
                head.push(text(" "));
                head.push(self.block(&b.statements, b.span));
                return cat(head);
            }
            other => other.as_expression().expect("an arrow's body is a block or an expression"),
        };
        let inner = match body {
            js::Expression::ParenthesizedExpression(p) => &p.expression,
            other => other,
        };
        if matches!(inner, js::Expression::ObjectExpression(_) | js::Expression::ArrayExpression(_) | js::Expression::TemplateLiteral(_) | js::Expression::ArrowFunctionExpression(_)) {
            head.push(text(" "));
            head.push(self.expr(body));
            return cat(head);
        }
        let printed = match body {
            js::Expression::BinaryExpression(_) | js::Expression::LogicalExpression(_) => self.binary(body, false),
            other => self.expr(other),
        };
        let tail = if last { cat(vec![if_break(","), soft()]) } else { text("") };
        group(cat(vec![cat(head), group(cat(vec![indent(cat(vec![line(), printed])), tail]))]))
    }

    fn call(&mut self, c: &js::CallExpression) -> Doc {
        let mut parts = vec![self.expr(&c.callee)];
        if c.optional {
            parts.push(text("?."));
        }
        if let Some(t) = &c.type_arguments {
            parts.push(self.verbatim(t.span));
        }
        parts.push(self.args(&c.arguments));
        cat(parts)
    }

    fn arg(&mut self, a: &js::Argument, last: bool) -> Doc {
        match a {
            js::Argument::SpreadElement(s) => cat(vec![text("..."), self.expr(&s.argument)]),
            other => match other.as_expression().expect("an argument is a spread or an expression") {
                js::Expression::ArrowFunctionExpression(f) => self.arrow(f, last),
                e => self.expr(e),
            },
        }
    }

    /// A call's arguments: on one line if they fit; else hugging the function passed (its body
    /// broken, what comes after it on its closing line) or the object passed last; else one to a
    /// line.
    fn args(&mut self, args: &[js::Argument]) -> Doc {
        if args.is_empty() {
            return text("()");
        }
        let n = args.len();
        let hug = self.hug(args);
        let mut printed = Vec::new();
        let mut expanded = None;
        for (i, a) in args.iter().enumerate() {
            // The function hugged last is laid out twice, as it is passed last or not, each placing
            // the same comments.
            if hug == Some(i) && i == n - 1 && matches!(a.as_expression(), Some(js::Expression::ArrowFunctionExpression(_))) {
                let before = self.used.clone();
                printed.push(self.arg(a, false));
                self.used = before;
                expanded = Some(self.arg(a, true));
            } else {
                printed.push(self.arg(a, false));
            }
        }
        let out = |items: Vec<Doc>| cat(vec![text("("), indent(cat(vec![soft(), cat(join(items, || cat(vec![text(","), line()])))])), if_break(","), soft(), text(")")]);
        let Some(h) = hug else { return group(out(printed)) };
        if printed.iter().enumerate().any(|(i, d)| i != h && will_break(d)) {
            return broken(out(printed));
        }
        let mut hugged_items = printed.clone();
        if let Some(e) = expanded {
            hugged_items[h] = e;
        }
        let flat = cat(vec![text("("), cat(join(hugged_items.clone(), || text(", "))), text(")")]);
        hugged_items[h] = broken(hugged_items[h].clone());
        let hugged = cat(vec![text("("), cat(join(hugged_items, || text(", "))), text(")")]);
        // An argument that will break breaks what holds the call too, as Prettier's does.
        let holds = if printed.iter().any(will_break) { Doc::BreakParent } else { text("") };
        cat(vec![holds, choice(vec![flat, hugged, broken(out(printed))])])
    }

    /// Which argument a call hugs, if one: the one function passed, last or followed by what isn't
    /// one (`route.post(path, (req) => { … }, { log })`, its body a block); with none, an object or
    /// array passed last.
    fn hug(&self, args: &[js::Argument]) -> Option<usize> {
        let n = args.len();
        let exprs: Vec<Option<&js::Expression>> = args.iter().map(|a| a.as_expression()).collect();
        let functions: Vec<usize> = (0..n).filter(|&i| exprs[i].is_some_and(is_function)).collect();
        match functions.as_slice() {
            [h] if *h == n - 1 => Some(*h),
            [h] if exprs[*h].is_some_and(block_bodied) => Some(*h),
            [] => {
                let last = exprs[n - 1]?;
                let expands = |e: &js::Expression| match e {
                    js::Expression::ObjectExpression(o) => !o.properties.is_empty(),
                    js::Expression::ArrayExpression(a) => !a.elements.is_empty(),
                    _ => false,
                };
                let before = n.checked_sub(2).and_then(|i| exprs[i]);
                (expands(last) && !before.is_some_and(expands)).then_some(n - 1)
            }
            _ => None,
        }
    }

    fn object(&mut self, o: &js::ObjectExpression) -> Doc {
        let (start, end) = (o.span.start + 1, o.span.end - 1);
        if o.properties.is_empty() {
            return self.empty_list("{", "}", start, end);
        }
        let open_broken = self.src[start as usize..o.properties[0].span().start as usize].contains('\n');
        let items = self.items(&o.properties, |p| p.span(), Self::property, start, end, true);
        let d = cat(vec![text("{"), indent(cat(vec![line(), items])), line(), text("}")]);
        if open_broken {
            broken(d)
        } else {
            group(d)
        }
    }

    fn property(&mut self, p: &js::ObjectPropertyKind) -> Doc {
        match p {
            js::ObjectPropertyKind::SpreadProperty(s) => cat(vec![text("..."), self.expr(&s.argument)]),
            js::ObjectPropertyKind::ObjectProperty(p) => {
                if p.method || p.kind != js::PropertyKind::Init {
                    return self.verbatim(p.span);
                }
                if p.shorthand {
                    return match &p.value {
                        js::Expression::Identifier(_) => self.expr(&p.value),
                        _ => self.verbatim(p.span),
                    };
                }
                cat(vec![self.key(&p.key, p.computed), text(": "), self.expr(&p.value)])
            }
        }
    }

    fn array(&mut self, a: &js::ArrayExpression) -> Doc {
        let (start, end) = (a.span.start + 1, a.span.end - 1);
        if a.elements.is_empty() {
            return self.empty_list("[", "]", start, end);
        }
        if a.elements.iter().any(|e| matches!(e, js::ArrayExpressionElement::Elision(_))) {
            return self.verbatim(a.span);
        }
        let items = self.items(&a.elements, |e| e.span(), Self::element, start, end, true);
        group(cat(vec![text("["), indent(cat(vec![soft(), items])), soft(), text("]")]))
    }

    fn element(&mut self, e: &js::ArrayExpressionElement) -> Doc {
        match e {
            js::ArrayExpressionElement::SpreadElement(s) => cat(vec![text("..."), self.expr(&s.argument)]),
            other => match other.as_expression() {
                Some(x) => self.expr(x),
                None => self.verbatim(other.span()),
            },
        }
    }

    /// An empty `{}` or `[]`, or one holding only comments.
    fn empty_list(&mut self, open: &str, close: &str, start: u32, end: u32) -> Doc {
        let notes = self.notes_in(start, end);
        if notes.is_empty() {
            return text(format!("{open}{close}"));
        }
        let inner: Vec<Doc> = notes.into_iter().map(|n| self.note(n)).collect();
        cat(vec![text(open), indent(cat(vec![hard(), cat(join(inner, hard))])), hard(), text(close)])
    }

    /// A comma list's items, for a group that lays them on one line or one to a line: each with the
    /// comments before it and on its line, a blank line kept where the text had one.
    fn items<T>(&mut self, list: &[T], span: impl Fn(&T) -> At, print: impl Fn(&mut Self, &T) -> Doc, start: u32, end: u32, comma: bool) -> Doc {
        let mut parts = Vec::new();
        let mut from = start;
        let mut last: Option<u32> = None;
        for (i, item) in list.iter().enumerate() {
            let at = span(item);
            if let Some(l) = last {
                parts.push(line());
                if self.blank(l, at.start) {
                    parts.push(soft());
                }
            }
            for n in self.notes_in(from, at.start) {
                parts.push(self.note(n));
                let own_line = n.line || self.src[n.end as usize..at.start as usize].contains('\n');
                parts.push(if own_line { hard() } else { text(" ") });
            }
            parts.push(print(self, item));
            if i + 1 < list.len() {
                parts.push(text(","));
            } else if comma {
                parts.push(if_break(","));
            }
            let till = list.get(i + 1).map_or(end, |n| span(n).start);
            let mut done = at.end;
            for n in self.trailing(at.end, till) {
                parts.push(Doc::Suffix(format!(" {}", self.at(At::new(n.start, n.end)))));
                parts.push(Doc::BreakParent);
                done = n.end;
            }
            last = Some(done);
            from = done;
        }
        for n in self.notes_in(from, end) {
            parts.push(hard());
            parts.push(self.note(n));
        }
        cat(parts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fmt's own layout of `src`, which must be what it already is.
    fn kept(src: &str) {
        let out = format(src).unwrap_or_else(|e| panic!("{e}\n{src}"));
        assert_eq!(out, src, "fmt changed it");
    }

    fn becomes(src: &str, expected: &str) {
        let out = format(src).unwrap_or_else(|e| panic!("{e}\n{src}"));
        assert_eq!(out, expected);
        assert_eq!(format(&out).unwrap(), out, "fmt is not idempotent on its own output");
    }

    const HEAD: &str = "import { flow, js, respond, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"f\", { port: 8000 })\n";

    const PAGE: &str = "\nconst page = js(function html(count) {\n  return `<p>${count}</p>`\n})\n\n";

    #[test]
    fn a_text_as_fmt_writes_it_is_kept() {
        kept(&format!("{HEAD}\n// Counts.\nroute.post(\"/count\", (req) => {{\n  const n = (state.count ?? 0) + 1\n  state.count = n\n  return n\n}})\n"));
    }

    #[test]
    fn layout_is_canonical() {
        becomes(
            &format!("{HEAD}route.post( '/count',(req)=>{{ const n=(state.count??0)+1;\n\n\n state.count=n ; return n }} )\n"),
            &format!("{HEAD}route.post(\"/count\", (req) => {{\n  const n = (state.count ?? 0) + 1\n\n  state.count = n\n  return n\n}})\n"),
        );
    }

    #[test]
    fn var_and_loose_equality_are_rewritten() {
        becomes(
            &format!("{HEAD}route.post(\"/v\", (req) => {{\n  var a = req.json.a\n  var b = 1\n  if (a == 2) b = 3\n  return a != b\n}})\n"),
            &format!("{HEAD}route.post(\"/v\", (req) => {{\n  const a = req.json.a\n  let b = 1\n  if (a === 2) b = 3\n  return a !== b\n}})\n"),
        );
    }

    #[test]
    fn comments_stay_where_they_were() {
        kept(&format!(
            "{HEAD}\n// The count.\n// Two lines.\nroute.get(\"/count\", () => state.count ?? 0) // on its line\n\nroute.post(\"/count\", (req) => {{\n  // before\n  state.count = req.json.n // after\n\n  /* block */ return \"ok\"\n  // last\n}})\n"
        ));
    }

    #[test]
    fn a_comment_inside_an_expression_keeps_its_statement_as_written() {
        kept(&format!("{HEAD}\nroute.post(\"/c\", (req) => {{\n  state.count = add(req.json.n,  /* two */ 2)\n  return \"ok\"\n}})\n\nfunction add(a, b) {{\n  return a + b\n}}\n"));
    }

    #[test]
    fn a_js_body_is_never_touched() {
        kept(&format!("{HEAD}\nconst page = js(function html(count) {{\n    return `<p>${{count}}</p>`;   \n}})\n\nroute.get(\"/\", () => page(state.count ?? 0))\n"));
    }

    #[test]
    fn a_long_call_breaks_hugging_its_function_or_object() {
        becomes(
            &format!("{HEAD}{PAGE}route.get(\"/\", () => respond(page(state.ticks ?? 0), {{ headers: {{ \"content-type\": \"text/html; charset=utf-8\" }}, status: 200 }}))\n"),
            &format!("{HEAD}{PAGE}route.get(\"/\", () =>\n  respond(page(state.ticks ?? 0), {{\n    headers: {{ \"content-type\": \"text/html; charset=utf-8\" }},\n    status: 200,\n  }}),\n)\n"),
        );
    }

    #[test]
    fn a_handlers_options_stay_after_its_closing_brace() {
        kept(&format!("{HEAD}\nroute.post(\"/n\", (req) => {{\n  state.n = req.json.n\n  return \"ok\"\n}}, {{ log: \"lazy\" }})\n"));
    }

    /// A flow written any which way, every construct a handler holds, as fmt lays it out.
    #[test]
    fn a_flow_written_any_which_way() {
        becomes(include_str!("../../tests/fmt/messy.holon.ts"), include_str!("../../tests/fmt/messy.laid.holon.ts"));
    }

    #[test]
    fn an_object_the_text_broke_stays_broken() {
        kept(&format!("{HEAD}\nroute.get(\"/\", () => ({{\n  a: 1,\n  b: 2,\n}}))\n"));
        becomes(&format!("{HEAD}route.get(\"/\", () => ({{ a: 1,\n  b: 2 }}))\n"), &format!("{HEAD}route.get(\"/\", () => ({{ a: 1, b: 2 }}))\n"));
    }
}
