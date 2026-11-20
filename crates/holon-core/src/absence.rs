//! Where an absence began: why an effect a run held wasn't made (register item R99, the run's
//! skipped effects).
//!
//! In a flow, absence is how "not found" and "not this time" are said (LANGUAGE.md §6.3): a read
//! that finds nothing, a field a record lacks, and a condition that came out false all leave the
//! values after them absent, and an effect built from an absent value isn't made. To a reader of
//! a run these are different facts: the flow chose not to, or something it relied on gave
//! nothing. [`origin`] tells them apart, with one walk over the run's steps from the effect back
//! to the step where the absence began, and says which by a [`Level`].
//!
//! The walk reads the steps through [`Seen`]: which ran, which of their outputs gave nothing, and
//! whether a condition was `true`. It needs no values otherwise, so what it answers holds none:
//! a level, a step's path and its leaf's name. It goes back:
//!
//! - **through an effect's trigger first.** A trigger that didn't arrive means the statement
//!   wasn't reached, whatever its value would have been. A trigger fed by a choice is walked as
//!   a choice; one fed by a value itself is `if (x !== undefined)`'s tie: the flow tested it, a
//!   [`Level::Condition`] at that value.
//! - **into a choice that ran.** A `guard` whose condition arrived and wasn't `true` is the
//!   origin, a [`Level::Condition`]: its value side was never run (a branch a choice didn't need
//!   isn't), so walking into it, as explain once did, would blame a step that never ran. A
//!   condition that didn't arrive is walked into instead: with it absent neither branch runs
//!   (§6.4), and what made it absent is the reason. A condition that is false only because an
//!   arrival test (`same_value(x, x) ?? false`, what `try` tests each value it makes with) found
//!   `x` absent is seen through to `x`: a `try` didn't test, it caught. A `??` that both sides of
//!   gave nothing names the side that isn't a condition, so an if-else's write names the branch
//!   taken.
//! - **into the first input that gave nothing,** for any other step, in port order, up through a
//!   function's inputs and down into the function whose output gave nothing.
//! - **to the step that gave nothing on inputs that all arrived.** Its leaf says the level: a read
//!   of the host ([`Level::Read`]), a `throw` or a module that said why ([`Level::Failed`]), any
//!   other ([`Level::Missing`]).
//!
//! **What may be computed from a secret is never read.** Whether such a step gave anything, or was
//! `true`, can answer yes or no about the secret, one guess a run; so [`Seen`] answers
//! [`Look::Withheld`] for it, without the answer, and the walk's one move there is
//! [`Level::Withheld`], an origin that names no step. It asks only that way, before anything else
//! of the step: a `guard` whose condition is withheld is withheld, its value side unread (whether
//! that ran says which way the condition went); a step with a withheld input follows an input that
//! is clean and gave nothing if it has one, as that alone makes it give nothing, and is withheld
//! otherwise. So the answer is a function of what is clean in the run, the same whichever way a
//! secret went: an effect whose own output is withheld is said the same, a clean input that gave
//! nothing if its leaf can't run without it, else withheld, whether it was made (and refused by
//! the host) or not (`origin` never reads which).
//!
//! Its limits, said in each answer ([`Origin::unseen`]): a function applied by `map`, `fold` and
//! the other higher-order leaves isn't followed into (the run's steps don't keep its
//! applications), nor is a step the run's record doesn't keep (deeper than it follows steps), so
//! the origin is the call, as far as the record sees.

use crate::{Resolver, Unit, UnitBody, Value, ASKS_HOST, BOUNDARY, CALLS, PASSES_EXTRA, SOURCED, TRIGGER};

/// Why an effect wasn't made: the kind of the step its absence began at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Level {
    /// The flow's own test came out false: an `if`, a `?:`, a `switch`, an `if (x !== undefined)`
    /// or a `return` before it. In most runs; a run's record doesn't keep it by default
    /// ([`Level::kept`]), and a view works it out again.
    Condition,
    /// A read of the host found nothing: a file, a row, an item, a state key never set.
    Read,
    /// A value the run had lacked what was asked of it: a field, an item of a list.
    Missing,
    /// A step failed: a `throw`, a module or a `js()` function that said why.
    Failed,
    /// Where it began may be computed from a secret: not said.
    Withheld,
    /// The effect was made and the host refused it (not granted, holding a secret, a never-kept
    /// value or a credential). Never an answer of [`origin`]: the host says it.
    Refused,
}

impl Level {
    /// The level's word in a run's record and its views.
    pub fn word(self) -> &'static str {
        match self {
            Level::Condition => "condition",
            Level::Read => "read",
            Level::Missing => "missing",
            Level::Failed => "failed",
            Level::Withheld => "withheld",
            Level::Refused => "refused",
        }
    }

    /// Whether a run's record keeps a note at this level by default: all but the flow's own
    /// condition, which is so in most runs and is worked out again from the record.
    pub fn kept(self) -> bool {
        self != Level::Condition
    }
}

/// Where an absence began.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub level: Level,
    /// The step, by its path of child indices from the root (`[0, 4]`); empty where it is
    /// withheld.
    pub at: Vec<u32>,
    /// That step's leaf (`file_read`, `guard`), empty for a function's or where it is withheld.
    pub leaf: String,
    /// The walk stopped there because the run's steps don't go further (a function applied by a
    /// higher-order leaf, a step the record doesn't keep): as far as the record sees.
    pub unseen: bool,
}

/// What a step's output gave, as the walk may know it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// It ran and gave nothing.
    Nothing,
    /// It ran and gave a value.
    Something,
    /// It didn't run, or the record doesn't keep it.
    Unseen,
    /// It may be computed from a secret: which of the others it is isn't said.
    Withheld,
}

/// Whether a step's output was the Boolean `true`, as the walk may know it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Truth {
    True,
    /// It arrived, and wasn't `true`.
    NotTrue,
    /// It didn't run, or the record doesn't keep it.
    Unseen,
    /// It may be computed from a secret: whether it was isn't said.
    Withheld,
}

/// A run's steps, as the walk reads them. A step that may be computed from a secret answers
/// [`Look::Withheld`] and [`Truth::Withheld`], whatever it gave: the walk can't read it.
pub trait Seen {
    /// What output `port` of the step at `path` gave.
    fn gave(&self, path: &[u32], port: u32) -> Look;
    /// Whether output `port` of the step at `path` was `true`.
    fn truth(&self, path: &[u32], port: u32) -> Truth;
    /// Whether the step said why it gave nothing (a `throw`'s message, a module's error). Asked
    /// only of a step the walk read gave nothing, which is clean.
    fn said_why(&self, _path: &[u32]) -> bool {
        false
    }
}

/// Why the output `port` of the step at `at` (an effect: a sink, a call, a function holding
/// them) gave nothing, in the run whose steps `seen` reads, of the unit `root`. `None` where it
/// didn't: it gave a value, or it didn't run (a branch not taken), or it is a call that was made
/// and answered nothing. Where the output itself is withheld, an answer whether or not it gave
/// anything (the module's doc): a caller asks only of an effect that wasn't made, or was made
/// and refused, and says both the same.
pub fn origin(root: &Unit, resolver: &impl Resolver, seen: &impl Seen, at: &[u32], port: u32) -> Option<Origin> {
    let mut walk = Walk { root, resolver, seen, budget: WALK_BUDGET };
    match seen.gave(at, port) {
        Look::Something | Look::Unseen => None,
        Look::Withheld => Some(walk.beneath(at, port)),
        Look::Nothing => {
            let found = walk.why(at, port);
            // The output being a call whose inputs all arrived, it was made: it answered nothing,
            // and that is its answer. (A handler keeps a call bound on a statement as an output
            // of its own.)
            let (step, _) = walk.source(at, port);
            let asked = found.at == step && found.level == Level::Read && walk.leaf_name(&step).is_some_and(|n| CALLS.contains(&n));
            (!asked).then_some(found)
        }
    }
}

/// The leaves that give a value with an input absent: the choices, `default`, and those with
/// inputs a call may leave out. Any other leaf gives nothing when an input it is wired gave
/// nothing. A call is held when one is absent (eval.rs `held_when_absent`), so it is one of
/// those.
fn runs_without_an_input(name: &str) -> bool {
    matches!(name, "guard" | "coalesce" | "and" | "or" | "default")
        || (crate::primitives::has_optional_ports(name) && !CALLS.contains(&name))
        || PASSES_EXTRA.contains(&name)
}

/// How many steps a walk takes at most: a graph is acyclic and finite, so this only bounds a
/// unit a resolver hands out wrongly.
const WALK_BUDGET: usize = 100_000;

struct Walk<'a, R: Resolver, S: Seen> {
    root: &'a Unit,
    resolver: &'a R,
    seen: &'a S,
    budget: usize,
}

/// An output: the step's path and its port.
type Out = (Vec<u32>, u32);

impl<'a, R: Resolver, S: Seen> Walk<'a, R, S> {
    /// The unit at `path` from the root.
    fn unit_at(&self, path: &[u32]) -> Option<&'a Unit> {
        let mut unit = self.root;
        for &i in path {
            let UnitBody::Composite { children, .. } = &unit.body else { return None };
            unit = self.resolver.resolve(children.get(i as usize)?)?;
        }
        Some(unit)
    }

    fn leaf_name(&self, path: &[u32]) -> Option<&'a str> {
        match &self.unit_at(path)?.body {
            UnitBody::Primitive { name } => Some(name.as_str()),
            _ => None,
        }
    }

    /// What feeds input `port` of the step at `path`: a sibling's output, or, through the
    /// function's own input, what feeds that, up to the root's inputs (`None`, as an unwired one).
    fn feeder(&self, path: &[u32], port: u32) -> Option<Out> {
        let (&ci, parent) = path.split_last()?;
        let UnitBody::Composite { edges, .. } = &self.unit_at(parent)?.body else { return None };
        let e = edges.iter().find(|e| e.to_child == ci && e.to_port == port)?;
        if e.from_child == BOUNDARY {
            return self.feeder(parent, e.from_port);
        }
        let mut from = parent.to_vec();
        from.push(e.from_child);
        Some((from, e.from_port))
    }

    /// The leaf output `port` of the step at `path` comes straight from: down through each
    /// function's own output, to the leaf (or the function, where it is fed by its input).
    fn source(&self, path: &[u32], port: u32) -> Out {
        let mut out = (path.to_vec(), port);
        while let Some(UnitBody::Composite { edges, .. }) = self.unit_at(&out.0).map(|u| &u.body) {
            match edges.iter().find(|e| e.to_child == BOUNDARY && e.to_port == out.1) {
                Some(e) if e.from_child != BOUNDARY => {
                    out.0.push(e.from_child);
                    out.1 = e.from_port;
                }
                _ => break,
            }
        }
        out
    }

    fn look(&self, out: &Out) -> Look {
        self.seen.gave(&out.0, out.1)
    }

    /// The inputs of the step at `path` that are wired, trigger first: each with whether it is
    /// the trigger.
    fn inputs(&self, path: &[u32], unit: &Unit) -> Vec<(Out, bool)> {
        let trigger = unit.inputs.iter().position(|p| p.name == TRIGGER);
        let order = trigger.into_iter().chain((0..unit.inputs.len()).filter(|&p| Some(p) != trigger));
        order.filter_map(|p| self.feeder(path, p as u32).map(|f| (f, Some(p) == trigger))).collect()
    }

    /// The step at `path` gave nothing, or is withheld: the first of its inputs that is clean and
    /// gave nothing, followed; else the first withheld one that a clean step beneath it makes
    /// give nothing whatever the secret ([`Walk::beneath`]); `None` if there is neither, and
    /// whether an input was withheld. What is read is clean whichever way a secret went, so which
    /// is followed is too.
    fn clean_absent_input(&mut self, path: &[u32], unit: &Unit) -> (Option<Origin>, bool) {
        let mut withheld = Vec::new();
        for (f, trigger) in self.inputs(path, unit) {
            match self.look(&f) {
                Look::Nothing => return (Some(if trigger { self.untied(f) } else { self.why(&f.0, f.1) }), true),
                Look::Withheld => withheld.push(f),
                Look::Something | Look::Unseen => {}
            }
        }
        for f in &withheld {
            let o = self.beneath(&f.0, f.1);
            if o != WITHHELD {
                return (Some(o), true);
            }
        }
        (None, !withheld.is_empty())
    }

    /// Why output `port` of the step at `at`, which is withheld, would have given nothing: a
    /// clean input that gave nothing, where its leaf gives nothing without it, else withheld.
    /// Never what the output gave: the answer is the same whether it gave anything or not.
    fn beneath(&mut self, at: &[u32], port: u32) -> Origin {
        if self.budget == 0 {
            return WITHHELD;
        }
        self.budget -= 1;
        let (leaf, _) = self.source(at, port);
        let Some(unit) = self.unit_at(&leaf) else { return WITHHELD };
        let UnitBody::Primitive { name } = &unit.body else { return WITHHELD };
        if name.as_str() == "guard" {
            // Either side clean and absent makes it absent; its value is read only once its
            // condition is (a value that ran says the condition was true).
            let Some(cond) = self.feeder(&leaf, 0) else { return WITHHELD };
            return match self.look(&cond) {
                Look::Nothing => self.why(&cond.0, cond.1),
                Look::Something => match self.seen.truth(&cond.0, cond.1) {
                    Truth::NotTrue => self.why_false(&cond, &leaf),
                    Truth::True => match self.feeder(&leaf, 1) {
                        Some(v) if self.look(&v) == Look::Nothing => self.why(&v.0, v.1),
                        _ => WITHHELD,
                    },
                    Truth::Unseen | Truth::Withheld => WITHHELD,
                },
                Look::Unseen | Look::Withheld => WITHHELD,
            };
        }
        if runs_without_an_input(name) {
            return WITHHELD;
        }
        self.clean_absent_input(&leaf, unit).0.unwrap_or(WITHHELD)
    }

    /// Why output `port` of the step at `path`, which gave nothing (read, so clean), did.
    fn why(&mut self, path: &[u32], port: u32) -> Origin {
        if self.budget == 0 {
            return self.stop(path, Level::Missing, true);
        }
        self.budget -= 1;
        let Some(unit) = self.unit_at(path) else { return self.stop(path, Level::Missing, true) };
        match &unit.body {
            // A function: the step inside that gave its output nothing, if the record keeps it.
            UnitBody::Composite { edges, .. } => {
                let Some(e) = edges.iter().find(|e| e.to_child == BOUNDARY && e.to_port == port) else { return self.own(path) };
                if e.from_child == BOUNDARY {
                    return match self.feeder(path, e.from_port) {
                        Some(f) => match self.look(&f) {
                            Look::Nothing => self.why(&f.0, f.1),
                            Look::Withheld => WITHHELD,
                            Look::Something | Look::Unseen => self.own(path),
                        },
                        None => self.own(path),
                    };
                }
                let mut inner = path.to_vec();
                inner.push(e.from_child);
                match self.seen.gave(&inner, e.from_port) {
                    Look::Nothing => self.why(&inner, e.from_port),
                    Look::Something => self.own(path),
                    Look::Unseen => self.stop(path, Level::Missing, true),
                    Look::Withheld => WITHHELD,
                }
            }
            UnitBody::Module { .. } => self.own(path),
            UnitBody::Primitive { name } => match name.as_str() {
                "guard" => self.guard(path),
                "coalesce" => {
                    // Its second side runs only when its first gave nothing: read in order, and
                    // a withheld side ends the walk before the other is read.
                    let mut sides = Vec::new();
                    for f in (0..2).filter_map(|p| self.feeder(path, p)).collect::<Vec<_>>() {
                        match self.look(&f) {
                            Look::Nothing => sides.push(self.why(&f.0, f.1)),
                            Look::Withheld => return WITHHELD,
                            Look::Something | Look::Unseen => {}
                        }
                    }
                    let pick = sides.iter().position(|o| o.level != Level::Condition).unwrap_or(0);
                    sides.into_iter().nth(pick).unwrap_or_else(|| self.own(path))
                }
                // A trigger that didn't arrive first (the statement wasn't reached), then the
                // inputs in port order: the first clean one that gave nothing. With none, a
                // withheld input may be why, and the step is withheld.
                _ => match self.clean_absent_input(path, unit) {
                    (Some(o), _) => o,
                    (None, true) => WITHHELD,
                    (None, false) => self.own(path),
                },
            },
        }
    }

    /// A statement's trigger fed by `f`, which gave nothing.
    fn untied(&mut self, f: Out) -> Origin {
        match self.leaf_name(&f.0) {
            Some("guard" | "coalesce" | "and" | "or") => self.why(&f.0, f.1),
            // Tied to a value itself: `if (x !== undefined)`, the flow's test of it.
            _ => self.stop(&f.0, Level::Condition, false),
        }
    }

    /// A `guard` at `path` that gave nothing. Its condition is read first, and alone: its value
    /// side ran only if the condition was true, so it is read only once the condition is.
    fn guard(&mut self, path: &[u32]) -> Origin {
        let Some(cond) = self.feeder(path, 0) else { return self.own(path) };
        match self.look(&cond) {
            Look::Nothing => return self.why(&cond.0, cond.1),
            Look::Withheld => return WITHHELD,
            Look::Something | Look::Unseen => {}
        }
        let value = |w: &Self| w.feeder(path, 1).map(|v| (w.look(&v), v));
        match self.seen.truth(&cond.0, cond.1) {
            Truth::True => match value(self) {
                Some((Look::Nothing, v)) => self.why(&v.0, v.1),
                Some((Look::Withheld, _)) => WITHHELD,
                _ => self.own(path),
            },
            Truth::NotTrue => self.why_false(&cond, path),
            Truth::Withheld => WITHHELD,
            Truth::Unseen => match value(self) {
                Some((Look::Nothing, v)) => self.why(&v.0, v.1),
                Some((Look::Withheld, _)) => WITHHELD,
                _ => self.stop(path, Level::Condition, false),
            },
        }
    }

    /// Why the condition `cond` of the guard at `guard` came out false: the flow's own test, but
    /// for an arrival test that found nothing, seen through to what didn't arrive.
    fn why_false(&mut self, cond: &Out, guard: &[u32]) -> Origin {
        let mut at = cond.clone();
        loop {
            match self.leaf_name(&at.0) {
                // `a && b`: the input that was false; `a || b`: each was, the first not a test.
                // In port order, as they ran (each after one that didn't decide): a withheld one
                // ends the walk before any after it is read.
                Some("and") => {
                    let n = self.unit_at(&at.0).map_or(0, |u| u.inputs.len() as u32);
                    let mut next = None;
                    for f in (0..n).filter_map(|p| self.feeder(&at.0, p)) {
                        match self.seen.truth(&f.0, f.1) {
                            Truth::NotTrue => {
                                next = Some(f);
                                break;
                            }
                            Truth::Withheld => return WITHHELD,
                            Truth::True | Truth::Unseen => {}
                        }
                    }
                    match next {
                        Some(f) => at = f,
                        None => break,
                    }
                }
                Some("or") => {
                    let n = self.unit_at(&at.0).map_or(0, |u| u.inputs.len() as u32);
                    for f in (0..n).filter_map(|p| self.feeder(&at.0, p)).collect::<Vec<_>>() {
                        match self.seen.truth(&f.0, f.1) {
                            Truth::NotTrue => {
                                let o = self.why_false(&f, guard);
                                if o.level != Level::Condition {
                                    return o;
                                }
                            }
                            Truth::Withheld => return WITHHELD,
                            Truth::True | Truth::Unseen => {}
                        }
                    }
                    break;
                }
                // `x ?? false`: the flow's fallback, unless what fell back is an arrival test.
                Some("default") => match self.feeder(&at.0, 0) {
                    Some(f) => match self.look(&f) {
                        Look::Nothing if self.arrival_test(&f.0) => match self.feeder(&f.0, 0) {
                            Some(x) => match self.look(&x) {
                                Look::Nothing => return self.why(&x.0, x.1),
                                Look::Withheld => return WITHHELD,
                                Look::Something | Look::Unseen => break,
                            },
                            None => break,
                        },
                        Look::Withheld => return WITHHELD,
                        _ => break,
                    },
                    None => break,
                },
                _ => break,
            }
        }
        self.stop(guard, Level::Condition, false)
    }

    /// `same_value(x, x)`: whether `x` arrived, as `try` and an `after` test it.
    fn arrival_test(&self, path: &[u32]) -> bool {
        self.leaf_name(path) == Some("same_value") && self.feeder(path, 0).is_some_and(|a| Some(a) == self.feeder(path, 1))
    }

    /// The step at `path` gave nothing on inputs that all arrived: its leaf says why.
    fn own(&self, path: &[u32]) -> Origin {
        let Some(unit) = self.unit_at(path) else { return self.stop(path, Level::Missing, true) };
        let applies = |name: &str| PASSES_EXTRA.contains(&name) && unit.params.iter().any(|p| p.name() == "fn" && p.value().is_some_and(|f| !f.is_empty()));
        let (level, unseen) = match &unit.body {
            UnitBody::Primitive { name } if name == "fail" => (Level::Failed, false),
            _ if self.seen.said_why(path) => (Level::Failed, false),
            UnitBody::Primitive { name } if ASKS_HOST.contains(&name.as_str()) || SOURCED.contains(&name.as_str()) => (Level::Read, false),
            UnitBody::Primitive { name } if applies(name) => (Level::Missing, true),
            UnitBody::Primitive { .. } => (Level::Missing, false),
            UnitBody::Module { .. } => (Level::Failed, false),
            UnitBody::Composite { .. } => (Level::Missing, true),
        };
        self.stop(path, level, unseen)
    }

    /// The origin at `path`: a step the walk read, so clean.
    fn stop(&self, path: &[u32], level: Level, unseen: bool) -> Origin {
        Origin { level, at: path.to_vec(), leaf: self.leaf_name(path).unwrap_or_default().to_string(), unseen }
    }
}

/// A withheld origin: it names no step.
pub const WITHHELD: Origin = Origin { level: Level::Withheld, at: Vec::new(), leaf: String::new(), unseen: false };

/// Whether a step's output is `true`, as a [`Seen::is_true`] over the values it has reads it.
pub fn is_true(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Boolean(true)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{eval_observed, hex, interface_of, Edge, Hash, MapResolver, Observer, Param, Port, Slot, Type};
    use std::collections::HashMap;

    /// A run's steps, as the evaluator told them, cut at `depth` as a record that keeps steps that
    /// deep is; with the steps marked as computed from a secret.
    #[derive(Default)]
    struct Steps {
        slots: HashMap<Vec<u32>, Vec<Slot>>,
        why: HashMap<Vec<u32>, String>,
        depth: Option<usize>,
        secret: Vec<Vec<u32>>,
    }

    impl Observer for Steps {
        fn produced(&mut self, path: &[u32], outputs: &[Slot]) {
            self.slots.insert(path.to_vec(), outputs.to_vec());
        }
        fn failed(&mut self, path: &[u32], reason: &str) {
            self.why.insert(path.to_vec(), reason.to_string());
        }
    }

    impl Steps {
        /// The step's outputs, as a record kept them: `Err` where it is withheld.
        fn kept(&self, path: &[u32]) -> Result<Option<&Vec<Slot>>, ()> {
            if self.secret.iter().any(|s| s == path) {
                return Err(());
            }
            if self.depth.is_some_and(|d| path.len() > d) {
                return Ok(None);
            }
            Ok(self.slots.get(path))
        }
        /// Whether the step gave nothing, read past what a secret withholds: for a test to
        /// check what ran.
        fn raw(&self, path: &[u32], port: u32) -> Option<bool> {
            self.slots.get(path).map(|s| matches!(s.get(port as usize), Some(Slot::Absent)))
        }
    }

    impl Seen for Steps {
        fn gave(&self, path: &[u32], port: u32) -> Look {
            match self.kept(path) {
                Err(()) => Look::Withheld,
                Ok(None) => Look::Unseen,
                Ok(Some(s)) if matches!(s.get(port as usize), Some(Slot::Absent)) => Look::Nothing,
                Ok(Some(_)) => Look::Something,
            }
        }
        fn truth(&self, path: &[u32], port: u32) -> Truth {
            match self.kept(path) {
                Err(()) => Truth::Withheld,
                Ok(None) => Truth::Unseen,
                Ok(Some(s)) if is_true(s.get(port as usize).and_then(Slot::value)) => Truth::True,
                Ok(Some(_)) => Truth::NotTrue,
            }
        }
        fn said_why(&self, path: &[u32]) -> bool {
            self.why.contains_key(path)
        }
    }

    /// A flow being built: its leaves in a resolver, its children and wires.
    struct Flow {
        r: MapResolver,
        children: Vec<Hash>,
        edges: Vec<Edge>,
    }

    fn port(name: &str) -> Port {
        Port { name: name.into(), ty: Type::Any }
    }

    impl Flow {
        fn new() -> Flow {
            Flow { r: MapResolver::new(), children: Vec::new(), edges: Vec::new() }
        }
        /// A leaf, with these params, fed by these outputs (child, port) in port order: its index.
        fn leaf(&mut self, name: &str, params: &[(&str, &str)], from: &[u32]) -> u32 {
            let mut unit = interface_of(name).unwrap_or_else(|| panic!("{name}"));
            for (k, v) in params {
                unit.params.retain(|p| p.name() != *k);
                unit.params.push(Param::frozen(*k, *v));
            }
            self.place(unit, from)
        }
        /// A sink tied to a trigger, as a statement's write is.
        fn sink(&mut self, name: &str, params: &[(&str, &str)], value: u32, trigger: u32) -> u32 {
            let mut unit = interface_of(name).unwrap();
            for (k, v) in params {
                unit.params.retain(|p| p.name() != *k);
                unit.params.push(Param::frozen(*k, *v));
            }
            unit.inputs.push(port(TRIGGER));
            self.place(unit, &[value, trigger])
        }
        fn place(&mut self, unit: Unit, from: &[u32]) -> u32 {
            let at = self.children.len() as u32;
            self.children.push(self.r.insert(unit).unwrap());
            for (p, &f) in from.iter().enumerate() {
                self.edges.push(Edge { from_child: f, from_port: 0, to_child: at, to_port: p as u32 });
            }
            at
        }
        fn konst(&mut self, value: &str, ty: &str) -> u32 {
            self.leaf("const", &[("value", value), ("type", ty)], &[])
        }
        /// The root's one input, the request record: wired from the boundary.
        fn input(&mut self) -> u32 {
            BOUNDARY
        }
        /// The flow as a unit exposing `outs`, run on `request`: its steps, and `origin` of each.
        fn run(self, outs: &[u32], request: Value, steps: Steps) -> (Unit, MapResolver, Steps) {
            let mut edges = self.edges;
            for (i, &o) in outs.iter().enumerate() {
                edges.push(Edge { from_child: o, from_port: 0, to_child: BOUNDARY, to_port: i as u32 });
            }
            let root = Unit {
                body: UnitBody::Composite { children: self.children, edges },
                params: vec![],
                inputs: vec![port("request")],
                outputs: outs.iter().map(|_| port("out")).collect(),
            };
            let mut steps = steps;
            eval_observed(&root, &[Some(request)], &self.r, &mut steps);
            (root, self.r, steps)
        }
    }

    fn record(fields: &[(&str, Value)]) -> Value {
        Value::Record(fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    /// Why the step `at` gave nothing.
    fn why(f: Flow, at: u32, request: Value) -> Option<Origin> {
        why_with(f, at, request, Steps::default())
    }

    fn why_with(f: Flow, at: u32, request: Value, steps: Steps) -> Option<Origin> {
        let (root, r, steps) = f.run(&[at], request, steps);
        origin(&root, &r, &steps, &[at], 0)
    }

    fn at(level: Level, path: &[u32], leaf: &str) -> Option<Origin> {
        Some(Origin { level, at: path.to_vec(), leaf: leaf.into(), unseen: false })
    }

    /// `state.a = notes[req.json.k]` with no such item: the read found nothing.
    #[test]
    fn a_read_that_found_nothing() {
        let mut f = Flow::new();
        let req = f.input();
        let k = f.leaf("field", &[("name", "k")], &[req]);
        let got = f.leaf("file_read", &[("folder", "notes")], &[k]);
        let w = f.sink("store_write", &[("key", "a")], got, req);
        assert_eq!(why(f, w, record(&[("k", Value::Text("gone.txt".into()))])), at(Level::Read, &[got], "file_read"));
    }

    /// `state.who = req.json.who`, sent without `who`: a value lacked a field.
    #[test]
    fn a_field_that_isnt_there() {
        let mut f = Flow::new();
        let req = f.input();
        let who = f.leaf("field", &[("name", "who")], &[req]);
        let w = f.sink("store_write", &[("key", "who")], who, req);
        assert_eq!(why(f, w, record(&[("other", Value::Number(1.0))])), at(Level::Missing, &[who], "field"));
    }

    /// `if (req.json.go) state.c = 1` with `go` false: the flow's own condition, at its guard, and
    /// not the value side the guard never ran (explain once said "const didn't run").
    #[test]
    fn an_if_not_taken_is_the_flows_condition() {
        let mut f = Flow::new();
        let req = f.input();
        let one = f.konst("1", "Number");
        let go = f.leaf("field", &[("name", "go")], &[req]);
        let truthy = f.leaf("is_truthy", &[], &[go]);
        let g = f.leaf("guard", &[], &[truthy, one]);
        let w = f.sink("store_write", &[("key", "c")], g, req);
        let (root, r, steps) = f.run(&[w], record(&[("go", Value::Boolean(false))]), Steps::default());
        assert_eq!(steps.raw(&[one], 0), None, "the value side wasn't run");
        assert_eq!(origin(&root, &r, &steps, &[w], 0), at(Level::Condition, &[g], "guard"));
    }

    /// The same `if` sent without `go`: neither branch runs (§6.4), and what made the condition
    /// absent is why.
    #[test]
    fn an_if_on_nothing_names_what_gave_nothing() {
        let mut f = Flow::new();
        let req = f.input();
        let one = f.konst("1", "Number");
        let go = f.leaf("field", &[("name", "go")], &[req]);
        let truthy = f.leaf("is_truthy", &[], &[go]);
        let g = f.leaf("guard", &[], &[truthy, one]);
        let w = f.sink("store_write", &[("key", "c")], g, req);
        assert_eq!(why(f, w, record(&[])), at(Level::Missing, &[go], "field"));
    }

    /// `if (n !== undefined) state.b = n`: the write is tied to `n`, which the flow tested.
    #[test]
    fn a_tie_to_a_value_is_the_flows_test() {
        let mut f = Flow::new();
        let req = f.input();
        let k = f.leaf("field", &[("name", "k")], &[req]);
        let n = f.leaf("file_read", &[], &[k]);
        let w = f.sink("store_write", &[("key", "b")], n, n);
        assert_eq!(why(f, w, record(&[("k", Value::Text("x".into()))])), at(Level::Condition, &[n], "file_read"));
    }

    /// `try { state.d = notes[k] } catch { … }`: the write is guarded by `ok`, false because the
    /// arrival test found the read gave nothing. The try caught; what it caught is the read.
    #[test]
    fn a_try_names_what_failed_in_it() {
        let mut f = Flow::new();
        let req = f.input();
        let k = f.leaf("field", &[("name", "k")], &[req]);
        let n = f.leaf("file_read", &[], &[k]);
        let same = f.leaf("same_value", &[], &[n, n]);
        let ok = f.leaf("default", &[("value", "false"), ("type", "Boolean")], &[same]);
        let g = f.leaf("guard", &[], &[ok, n]);
        let w = f.sink("store_write", &[("key", "d")], g, req);
        assert_eq!(why(f, w, record(&[("k", Value::Text("x".into()))])), at(Level::Read, &[n], "file_read"));
    }

    /// Two values tried, `ok = a_arrived && b_arrived`: the one that didn't arrive.
    #[test]
    fn a_try_of_two_names_the_one_that_failed() {
        let mut f = Flow::new();
        let req = f.input();
        let a = f.leaf("field", &[("name", "a")], &[req]);
        let b = f.leaf("field", &[("name", "b")], &[req]);
        let sa = f.leaf("same_value", &[], &[a, a]);
        let sb = f.leaf("same_value", &[], &[b, b]);
        let oka = f.leaf("default", &[("value", "false"), ("type", "Boolean")], &[sa]);
        let okb = f.leaf("default", &[("value", "false"), ("type", "Boolean")], &[sb]);
        let ok = f.leaf("and", &[], &[oka, okb]);
        let g = f.leaf("guard", &[], &[ok, a]);
        let w = f.sink("store_write", &[("key", "d")], g, req);
        assert_eq!(why(f, w, record(&[("a", Value::Number(1.0))])), at(Level::Missing, &[b], "field"));
    }

    /// `if (req.json.flag ?? false) state.e = 1` without `flag`: the flow gave its own fallback,
    /// so its test came out false, a condition.
    #[test]
    fn a_fallback_the_flow_wrote_is_its_condition() {
        let mut f = Flow::new();
        let req = f.input();
        let one = f.konst("1", "Number");
        let flag = f.leaf("field", &[("name", "flag")], &[req]);
        let c = f.leaf("default", &[("value", "false"), ("type", "Boolean")], &[flag]);
        let g = f.leaf("guard", &[], &[c, one]);
        let w = f.sink("store_write", &[("key", "e")], g, req);
        assert_eq!(why(f, w, record(&[])), at(Level::Condition, &[g], "guard"));
    }

    /// `if (c) state.k = a; else state.k = b`, `c` true and `a` missing: the branch taken, not
    /// the else's guard, which was false.
    #[test]
    fn an_if_else_names_the_branch_taken() {
        let mut f = Flow::new();
        let req = f.input();
        let c = f.konst("true", "Boolean");
        let a = f.leaf("field", &[("name", "a")], &[req]);
        let b = f.konst("2", "Number");
        let not_c = f.leaf("not", &[], &[c]);
        let ga = f.leaf("guard", &[], &[c, a]);
        let gb = f.leaf("guard", &[], &[not_c, b]);
        let either = f.leaf("coalesce", &[], &[ga, gb]);
        let w = f.sink("store_write", &[("key", "k")], either, req);
        assert_eq!(why(f, w, record(&[])), at(Level::Missing, &[a], "field"));
    }

    /// `throw new Error("no")`, its value written: the throw failed it.
    #[test]
    fn a_throw_is_a_failure() {
        let mut f = Flow::new();
        let req = f.input();
        let m = f.konst("no", "Text");
        let t = f.leaf("fail", &[], &[m]);
        let w = f.sink("store_write", &[("key", "k")], t, req);
        assert_eq!(why(f, w, record(&[])), at(Level::Failed, &[t], "fail"));
    }

    /// A read the step marks as computed from a secret: the origin names no step.
    #[test]
    fn an_origin_computed_from_a_secret_is_withheld() {
        let mut f = Flow::new();
        let req = f.input();
        let k = f.leaf("field", &[("name", "k")], &[req]);
        let got = f.leaf("file_read", &[], &[k]);
        let w = f.sink("store_write", &[("key", "a")], got, req);
        let steps = Steps { secret: vec![vec![got]], ..Steps::default() };
        let o = why_with(f, w, record(&[("k", Value::Text("x".into()))]), steps).unwrap();
        assert_eq!(o, Origin { level: Level::Withheld, at: vec![], leaf: String::new(), unseen: false });
    }

    /// A condition computed from a secret: whether it was true isn't read, nor said.
    #[test]
    fn a_condition_computed_from_a_secret_is_withheld() {
        let mut f = Flow::new();
        let req = f.input();
        let one = f.konst("1", "Number");
        let c = f.konst("false", "Boolean");
        let g = f.leaf("guard", &[], &[c, one]);
        let w = f.sink("store_write", &[("key", "c")], g, req);
        let steps = Steps { secret: vec![vec![c]], ..Steps::default() };
        assert_eq!(why_with(f, w, record(&[]), steps), Some(WITHHELD));
    }

    /// A call whose body didn't arrive wasn't sent: skipped, for the read. One whose inputs all
    /// arrived was sent, and answered nothing: not skipped.
    #[test]
    fn a_call_skipped_and_a_call_made() {
        let mut f = Flow::new();
        let req = f.input();
        let k = f.leaf("field", &[("name", "k")], &[req]);
        let body = f.leaf("file_read", &[], &[k]);
        let path = f.konst("/x", "Text");
        let call = f.leaf("http_request", &[("method", "POST")], &[]);
        f.edges.push(Edge { from_child: path, from_port: 0, to_child: call, to_port: 0 });
        f.edges.push(Edge { from_child: body, from_port: 0, to_child: call, to_port: 2 });
        assert_eq!(why(f, call, record(&[("k", Value::Text("x".into()))])), at(Level::Read, &[body], "file_read"));

        let mut f = Flow::new();
        let _ = f.input();
        let path = f.konst("/x", "Text");
        let call = f.leaf("http_request", &[], &[path]);
        assert_eq!(why(f, call, record(&[])), None, "it was asked, and the host answered nothing");
    }

    /// A handler keeps a call bound on a statement as an output of its own, beside its answer
    /// (the endpoint passes only its effects on, routes.rs `caller_unit`): walked from the
    /// handler's output, a call not sent is skipped, one sent and answered nothing isn't.
    #[test]
    fn a_call_a_handler_keeps() {
        let handler = |with_body: bool| {
            let mut f = Flow::new();
            let mut children = vec![
                f.r.insert(Unit { params: vec![Param::frozen("value", "/x"), Param::frozen("type", "Text")], ..interface_of("const").unwrap() }).unwrap(),
                f.r.insert(interface_of("http_request").unwrap()).unwrap(),
            ];
            let mut edges = vec![
                Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 },
                Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 },
            ];
            if with_body {
                children.push(f.r.insert(Unit { params: vec![Param::frozen("name", "k")], ..interface_of("field").unwrap() }).unwrap());
                children.push(f.r.insert(interface_of("file_read").unwrap()).unwrap());
                edges.push(Edge { from_child: BOUNDARY, from_port: 0, to_child: 2, to_port: 0 });
                edges.push(Edge { from_child: 2, from_port: 0, to_child: 3, to_port: 0 });
                edges.push(Edge { from_child: 3, from_port: 0, to_child: 1, to_port: 2 });
            }
            let unit = Unit { body: UnitBody::Composite { children, edges }, params: vec![], inputs: vec![port("request")], outputs: vec![port("sent")] };
            let req = f.input();
            let h = f.place(unit, &[req]);
            (f, h)
        };
        let (f, h) = handler(true);
        assert_eq!(why(f, h, record(&[("k", Value::Text("x".into()))])), at(Level::Read, &[h, 3], "file_read"));
        let (f, h) = handler(false);
        assert_eq!(why(f, h, record(&[])), None, "sent, and answered nothing");
    }

    /// An effect in a branch not taken isn't run at all: nothing to say, and nothing walked.
    #[test]
    fn an_effect_not_run_says_nothing() {
        let mut f = Flow::new();
        let req = f.input();
        let one = f.konst("1", "Number");
        let w = f.sink("store_write", &[("key", "k")], one, req);
        let no = f.konst("false", "Boolean");
        let g = f.leaf("guard", &[], &[no, w]);
        let (root, r, steps) = f.run(&[g], record(&[]), Steps::default());
        assert_eq!(steps.raw(&[w], 0), None);
        assert_eq!(origin(&root, &r, &steps, &[w], 0), None);
        assert_eq!(origin(&root, &r, &steps, &[g], 0), at(Level::Condition, &[g], "guard"));
    }

    /// A function of the flow's, `record(x)`, whose read found nothing: followed into it, and out
    /// through its input to the caller's field.
    fn function_flow() -> (Flow, u32) {
        let mut f = Flow::new();
        // fn(k) { state.a = notes[k.name] }: field, file_read, store_write (an open trigger).
        let inner = Unit {
            body: UnitBody::Composite {
                children: vec![
                    f.r.insert(Unit { params: vec![Param::frozen("name", "name")], ..interface_of("field").unwrap() }).unwrap(),
                    f.r.insert(interface_of("file_read").unwrap()).unwrap(),
                    f.r.insert(Unit { params: vec![Param::frozen("key", "a")], ..interface_of("store_write").unwrap() }).unwrap(),
                ],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 },
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 },
                    Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 0 },
                    Edge { from_child: 2, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                ],
            },
            params: vec![],
            inputs: vec![port("k")],
            outputs: vec![Port { name: "write".into(), ty: Type::Effect }],
        };
        let req = f.input();
        let call = f.place(inner, &[req]);
        (f, call)
    }

    #[test]
    fn a_function_is_followed_into() {
        let (f, call) = function_flow();
        assert_eq!(why(f, call, record(&[("name", Value::Text("x".into()))])), at(Level::Read, &[call, 1], "file_read"));
        let (f, call) = function_flow();
        assert_eq!(why(f, call, record(&[])), at(Level::Missing, &[call, 0], "field"));
    }

    /// Limit: a step deeper than the record keeps (`Timer::DEPTH`). The function call is the
    /// origin, as far as the record sees.
    #[test]
    fn a_step_the_record_doesnt_keep_stops_at_the_call() {
        let (f, call) = function_flow();
        let steps = Steps { depth: Some(1), ..Steps::default() };
        let o = why_with(f, call, record(&[("name", Value::Text("x".into()))]), steps).unwrap();
        assert_eq!(o, Origin { level: Level::Missing, at: vec![call], leaf: String::new(), unseen: true });
    }

    /// Limit: `map`'s applications aren't steps of the run: an item whose field is missing makes
    /// the map give nothing, and the map is the origin, as far as the record sees.
    #[test]
    fn a_map_is_as_far_as_the_record_sees() {
        let mut f = Flow::new();
        let req = f.input();
        let x = f.r.insert(Unit { params: vec![Param::frozen("name", "x")], ..interface_of("field").unwrap() }).unwrap();
        let items = f.leaf("field", &[("name", "items")], &[req]);
        let m = f.leaf("map", &[("fn", &hex(x))], &[items]);
        let w = f.sink("store_write", &[("key", "xs")], m, req);
        let o = why(f, w, record(&[("items", Value::List(vec![record(&[])]))])).unwrap();
        assert_eq!(o, Origin { level: Level::Missing, at: vec![m], leaf: "map".into(), unseen: true });
    }

    /// Limit: `fold` the same.
    #[test]
    fn a_fold_is_as_far_as_the_record_sees() {
        let mut f = Flow::new();
        let req = f.input();
        // fn(acc, item) { item.x }
        let step = Unit {
            body: UnitBody::Composite {
                children: vec![f.r.insert(Unit { params: vec![Param::frozen("name", "x")], ..interface_of("field").unwrap() }).unwrap()],
                edges: vec![Edge { from_child: BOUNDARY, from_port: 1, to_child: 0, to_port: 0 }, Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
            },
            params: vec![],
            inputs: vec![port("acc"), port("item")],
            outputs: vec![port("out")],
        };
        let step = f.r.insert(step).unwrap();
        let zero = f.konst("0", "Number");
        let items = f.leaf("field", &[("name", "items")], &[req]);
        let folded = f.leaf("fold", &[("fn", &hex(step))], &[zero, items]);
        let w = f.sink("store_write", &[("key", "sum")], folded, req);
        let o = why(f, w, record(&[("items", Value::List(vec![record(&[])]))])).unwrap();
        assert_eq!(o, Origin { level: Level::Missing, at: vec![folded], leaf: "fold".into(), unseen: true });
    }

    /// A flow with a test on a secret (`s`, a constant here, its step and what is computed from it
    /// marked as the secret's), built by `build` from `s`, the request and a read that finds
    /// nothing: its write's origin with the test true and with it false, each as the record
    /// withholds it, and as it would be read if nothing were withheld (`raw`).
    fn both_ways(build: impl Fn(&mut Flow, u32, u32, u32) -> (u32, Vec<u32>)) -> [(Option<Origin>, Option<bool>); 2] {
        ["true", "false"].map(|s| {
            let mut f = Flow::new();
            let req = f.input();
            let test = f.konst(s, "Boolean");
            let k = f.leaf("field", &[("name", "k")], &[req]);
            let none = f.leaf("file_read", &[], &[k]);
            let (w, mut secret) = build(&mut f, test, none, req);
            secret.push(test);
            let steps = Steps { secret: secret.into_iter().map(|p| vec![p]).collect(), ..Steps::default() };
            let (root, r, steps) = f.run(&[w], record(&[("k", Value::Text("gone.txt".into()))]), steps);
            (origin(&root, &r, &steps, &[w], 0), steps.raw(&[w], 0))
        })
    }

    /// Shape A: a write fed by a value computed from a secret on an earlier port and by a clean
    /// read that found nothing on a later one. The walk read the earlier one first: with it
    /// absent the origin was withheld, with it there the read. Now the clean read is followed
    /// either way, and the write, itself computed from the secret, is said the same whether it
    /// gave anything or not.
    #[test]
    fn an_input_computed_from_a_secret_is_never_read_before_a_clean_one() {
        let [one, other] = both_ways(|f, test, none, req| {
            let one = f.konst("1", "Number");
            let b = f.leaf("guard", &[], &[test, one]);
            let p = f.leaf("pair", &[], &[b, none]);
            let w = f.sink("store_write", &[("key", "w")], p, req);
            (w, vec![b, p, w])
        });
        assert_eq!(one.0, other.0);
        assert_eq!(one.0, at(Level::Read, &[2], "file_read"));
    }

    /// A guard whose condition is computed from a secret: its value side, which ran only if the
    /// condition was true, isn't read. Withheld either way.
    #[test]
    fn a_guards_value_is_not_read_before_its_condition() {
        let [one, other] = both_ways(|f, test, _none, req| {
            let v = f.leaf("field", &[("name", "v")], &[req]);
            let g = f.leaf("guard", &[], &[test, v]);
            let w = f.sink("store_write", &[("key", "w")], g, req);
            (w, vec![g, w])
        });
        assert_eq!((one.0.clone(), other.0.clone()), (Some(WITHHELD), Some(WITHHELD)));
        // The write gave nothing with either: the value missing, or the condition false.
        assert_eq!((one.1, other.1), (Some(true), Some(true)));
    }

    /// Shape B, in the walk: a write a secret's test decides is made with it true and not with
    /// it false; its origin is asked without reading which (the host refuses the one made).
    #[test]
    fn a_write_a_secret_decides_is_said_the_same_made_or_not() {
        let [one, other] = both_ways(|f, test, _none, req| {
            let one = f.konst("1", "Number");
            let g = f.leaf("guard", &[], &[test, one]);
            let w = f.sink("store_write", &[("key", "w")], g, req);
            (w, vec![g, w])
        });
        assert_eq!((one.1, other.1), (Some(false), Some(true)), "made with the test true, not with it false");
        assert_eq!((one.0.clone(), other.0.clone()), (Some(WITHHELD), Some(WITHHELD)));
    }

    /// `x ?? s`, `s ?? x`, `a && s`, `s || a`: a choice with a side computed from a secret.
    /// The same answer whichever way the secret went.
    #[test]
    fn a_choice_with_a_side_computed_from_a_secret_reads_the_same() {
        for (name, secret_first) in [("coalesce", true), ("coalesce", false), ("and", true), ("and", false), ("or", true), ("or", false)] {
            let [one, other] = both_ways(|f, test, none, req| {
                let clean = if name == "coalesce" { none } else { f.leaf("default", &[("value", "false"), ("type", "Boolean")], &[none]) };
                let side = if name == "coalesce" {
                    let one = f.konst("1", "Number");
                    f.leaf("guard", &[], &[test, one])
                } else {
                    test
                };
                let c = f.leaf(name, &[], &if secret_first { [side, clean] } else { [clean, side] });
                let mut secret = vec![side, c];
                let w = if name == "coalesce" {
                    f.sink("store_write", &[("key", "w")], c, req)
                } else {
                    let one = f.konst("1", "Number");
                    let g = f.leaf("guard", &[], &[c, one]);
                    secret.push(g);
                    f.sink("store_write", &[("key", "w")], g, req)
                };
                secret.push(w);
                (w, secret)
            });
            assert_eq!(one.0, other.0, "{name}, the secret's side first: {secret_first}");
        }
    }

    /// The level's words and which a record keeps by default.
    #[test]
    fn a_record_keeps_all_but_the_condition() {
        let all = [Level::Condition, Level::Read, Level::Missing, Level::Failed, Level::Withheld, Level::Refused];
        assert_eq!(all.map(Level::word), ["condition", "read", "missing", "failed", "withheld", "refused"]);
        assert_eq!(all.map(Level::kept), [false, true, true, true, true, true]);
    }
}
