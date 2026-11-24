//! Where each doing step of a text is reached, worked out in one place, beside the graph's own
//! ways of holding a step back (step C1 of the one rule for reached): recorded, compared with what
//! a run does today, and used by nothing else yet. No document, unit or hash depends on it.
//!
//! A doing site is a place in the text that does something when the run gets there: a write, a
//! delete, a send, a log line, a request, an action started or another flow called, a wait, a
//! read, a function of the flow's entered. **A site is reached when every atom of its path holds,
//! looked at in the order the run meets them, each only when every one before it holds**, as
//! JavaScript runs a body: a handler's run began ([`Atom::Entry`]), or a call of the function was
//! reached ([`Atom::Entered`]), or the list method applied the function written in place to the
//! item ([`Atom::Item`]); then each condition around the site and each statement before it, in
//! the order written ([`Atom::Holds`], [`Atom::Completed`]).
//!
//! The order is part of the rule, not a detail of how the path is kept: a condition after a
//! `return` that may have answered, or after a statement that failed, is never looked at by the
//! run, and so is never asked to have a value (a request it would read was never made). Read in
//! another order, a path would call a site unreached for a test the run never made. The Reader
//! keeps the path in the order it reads the text, which is that order ([`Site::path`]).
//!
//! A statement that "did not complete" stops what follows only inside a `try`: there, a value it
//! made that didn't arrive is the block failing ([`Why::Arrived`]). Outside a `try`, a flow today
//! reads a line that gives nothing as a value missing, not a failure: what follows and doesn't use
//! it runs, and what uses it has nothing to go on. JavaScript throws at such a line (`JSON.parse`
//! of a text that isn't JSON, a request that failed), and nothing below it runs. Whether that is
//! Holon's own rule or owed to JavaScript is the owner's open question; one switch labels it
//! (`reached_runs::PAST_NOTHING`). So [`Atom::Completed`] always holds, as today; it is recorded so
//! that a run where JavaScript would have stopped there can be labelled. A `try`'s own
//! promise, that a failed block keeps none of its writes, is not part of reached either: it is
//! [`Site::kept`], beside it.

use super::ast::{Expr, Span};

/// One doing site of a text, and the path that reaches it.
#[derive(Clone, Debug, PartialEq)]
pub struct Site {
    /// Where it is written: the statement's or the expression's span.
    pub at: Span,
    /// What it does, in a word: `writes`, `deletes`, `sends`, `logs`, `requests`, `starts`,
    /// `calls`, `enters`, `waits`, `reads` (a collection, a folder, a database), `reads state`,
    /// `throws`.
    pub does: &'static str,
    /// The handler's or the function's body it is in.
    pub within: Span,
    /// What must hold for the run to get there, in the order the run meets it.
    pub path: Vec<Atom>,
    /// What it is given and can't run without: a write's value (and key), a request's, a send's
    /// or a call's arguments. Where one has no value, what needs it doesn't run (Holon's rule for
    /// a value missing), where JavaScript runs it with `undefined`.
    pub needs: Vec<Expr>,
    /// For a write to state, its key: the graph writes each key once per run, at one step for
    /// every place the text writes it (the one-key-many-writes column, beside reached).
    pub key: Option<String>,
    /// For a site inside a `try` block with a catch, the block's own test (`ok`): what it did is
    /// kept only where that holds. Innermost last.
    pub kept: Vec<Expr>,
}

/// One thing a path needs, in the order the run meets them.
#[derive(Clone, Debug, PartialEq)]
pub enum Atom {
    /// The handler's run began.
    Entry,
    /// A function's body: entered where a call of it (a site that `enters` it) is reached.
    Entered(String),
    /// A function written in place, applied to an item by the list method written at `at`.
    Item { method: String, at: Span },
    /// `test` has a value and it is true, as JavaScript reads a condition; why it is on the path.
    Holds { why: Why, test: Expr },
    /// The statement written at the span, outside a `try`, before the site: it always counts as
    /// completed (as today, above), where JavaScript stops if a step in it threw.
    Completed(Span),
}

/// Why a test is on a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// An `if`'s branch, a case of a `switch`, a `catch` (the block's test not holding).
    Branch,
    /// A `return` before it not taken: the return's condition has a value, and it is false.
    NotReturned,
    /// In a `try` block, a value made before it arrived.
    Arrived,
    /// `if (x !== undefined)`: `x` has a value.
    Present,
    /// An arm of `&&`, `||`, `??` or `?:`: the part before it lets it run.
    Arm,
    /// Past a wait: the wait was answered.
    Answered,
}

/// What a test's part is in a run: a value, nothing, or not known (the run never computed it).
#[derive(Clone, Debug, PartialEq)]
pub enum Seen {
    Value(holon_core::Value),
    Absent,
    Unknown,
}

/// JavaScript's ToBoolean of a value.
pub fn truthy(v: &holon_core::Value) -> bool {
    use holon_core::Value as V;
    match v {
        V::Boolean(b) => *b,
        V::Number(n) => *n != 0.0 && !n.is_nan(),
        V::Text(t) => !t.is_empty(),
        _ => true,
    }
}

/// The parts of `test` a run computes: what is left of it under the few forms the Reader makes
/// tests of (`!`, `&&`, `||`, `??`, `when`, `?:`, `===`, `same_value`, literals), which
/// [`value`] reads itself (a `&&` or `||` the run computed whole, read as it computed it).
pub fn parts(test: &Expr, out: &mut Vec<Expr>) {
    use super::ast::{BinOp, ExprKind as K};
    match &test.kind {
        K::Bool(_) | K::Number { .. } | K::Text(_) => {}
        K::Paren(x) | K::Not(x) | K::Truthy(x) => parts(x, out),
        K::Binary { op: op @ (BinOp::And | BinOp::Or | BinOp::Coalesce | BinOp::When | BinOp::Eq | BinOp::Ne), lhs, rhs } => {
            // `&&` and `||` are also read whole where the run computed them: as a value they take
            // Booleans (Holon's reading), which their forms alone can't tell from a condition's.
            if matches!(op, BinOp::And | BinOp::Or) && !out.contains(test) {
                out.push(test.clone());
            }
            parts(lhs, out);
            parts(rhs, out);
        }
        K::If { cond, then, els } => {
            parts(cond, out);
            parts(then, out);
            if let Some(e) = els {
                parts(e, out);
            }
        }
        K::Leaf { leaf, args, .. } if leaf.text == "same_value" || leaf.text == "has_value" => args.iter().for_each(|a| parts(&a.value, out)),
        K::List(xs) => xs.iter().for_each(|x| parts(x, out)),
        _ => {
            if !out.contains(test) {
                out.push(test.clone());
            }
        }
    }
}

/// What `test` gives, its [`parts`] read by `part`, each looked at only where the forms around
/// it look at it, as the run does: the right of `&&` only where the left holds.
pub fn value(test: &Expr, part: &mut dyn FnMut(&Expr) -> Seen) -> Seen {
    use super::ast::{BinOp, ExprKind as K};
    use holon_core::Value as V;
    let boolean = |b: bool| Seen::Value(V::Boolean(b));
    match &test.kind {
        K::Bool(b) => boolean(*b),
        K::Number { value: n, .. } => Seen::Value(V::Number(*n)),
        K::Text(t) => Seen::Value(V::Text(t.clone())),
        K::Paren(x) => value(x, part),
        K::Not(x) => match value(x, part) {
            Seen::Value(v) => boolean(!truthy(&v)),
            other => other,
        },
        K::Truthy(x) => match value(x, part) {
            Seen::Value(v) => boolean(truthy(&v)),
            other => other,
        },
        K::Binary { op, lhs, rhs } => match op {
            BinOp::And | BinOp::Or if part(test) != Seen::Unknown => part(test),
            BinOp::And | BinOp::Or => match value(lhs, part) {
                Seen::Value(v) if truthy(&v) == (*op == BinOp::Or) => boolean(truthy(&v)),
                Seen::Value(_) => value(rhs, part),
                other => other,
            },
            BinOp::Coalesce => match value(lhs, part) {
                Seen::Absent => value(rhs, part),
                other => other,
            },
            BinOp::When => match value(rhs, part) {
                Seen::Value(c) if truthy(&c) => value(lhs, part),
                Seen::Unknown => Seen::Unknown,
                _ => Seen::Absent,
            },
            BinOp::Eq | BinOp::Ne => match (value(lhs, part), value(rhs, part)) {
                (Seen::Value(a), Seen::Value(b)) => boolean((a == b) == (*op == BinOp::Eq)),
                (Seen::Unknown, _) | (_, Seen::Unknown) => Seen::Unknown,
                _ => Seen::Absent,
            },
            _ => part(test),
        },
        K::If { cond, then, els } => match value(cond, part) {
            Seen::Value(c) if truthy(&c) => value(then, part),
            Seen::Value(_) => els.as_ref().map_or(Seen::Absent, |e| value(e, part)),
            other => other,
        },
        K::Leaf { leaf, args, .. } if leaf.text == "has_value" && args.len() == 1 => match value(&args[0].value, part) {
            Seen::Value(_) => boolean(true),
            Seen::Absent => boolean(false),
            Seen::Unknown => Seen::Unknown,
        },
        K::Leaf { leaf, args, .. } if leaf.text == "same_value" && args.len() == 2 => match (value(&args[0].value, part), value(&args[1].value, part)) {
            (Seen::Value(a), Seen::Value(b)) => boolean(a == b || matches!((&a, &b), (V::Number(x), V::Number(y)) if x.is_nan() && y.is_nan())),
            (Seen::Unknown, _) | (_, Seen::Unknown) => Seen::Unknown,
            _ => Seen::Absent,
        },
        K::List(xs) => {
            let mut items = Vec::new();
            for x in xs {
                match value(x, part) {
                    Seen::Value(v) => items.push(v),
                    other => return other,
                }
            }
            Seen::Value(V::List(items))
        }
        _ => part(test),
    }
}

/// Whether a test holds: it has a value, and the value is true as JavaScript reads it; `None`
/// where the run never computed what it needs.
pub fn holds(test: &Expr, part: &mut dyn FnMut(&Expr) -> Seen) -> Option<bool> {
    match value(test, part) {
        Seen::Value(v) => Some(truthy(&v)),
        Seen::Absent => Some(false),
        Seen::Unknown => None,
    }
}
