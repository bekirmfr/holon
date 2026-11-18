//! The effects a run held and didn't make, for its record (register item R99).
//!
//! A handler holds each effect its text makes (a write, a send, a log line, a call, an action
//! started) as an output of its own, and the host performs those that arrive (`perform_effects`)
//! and makes the calls that do. One that didn't arrive was dropped without a word: the run looked
//! clean and nothing was written. Here each such effect is said, with where its absence began
//! (core's `absence::origin`): a read that found nothing, a value that lacked a field, a step that
//! failed, or, where that may be computed from a secret, withheld. One the flow's own condition
//! left out (an `if` not taken, an early return) is so in most runs, and isn't kept: the views
//! work it out again from the record. An effect made and refused by the host is said beside them,
//! by the rule that refused it.
//!
//! What it says is the same whichever way a secret went (two runs that differ only in what is
//! computed from one, everything clean the same, read the same): the walk never reads a step that
//! may be computed from a secret (`Steps`, below, answers withheld for it), and an effect that may
//! carry something computed from one is said the same whether it wasn't made or was made and
//! refused: withheld, naming nothing. The notes are kept in one order, by what they say, so which
//! of the two happened doesn't show in where a note stands either. Two such runs can still differ
//! here where the record already says they differ: an effect made and kept in one (a call sent, a
//! line written, an action started) and not in the other.
//!
//! It costs a run nothing unless an absence began in it (core's `Observer::began_absent`, heard
//! by the run's `Timer`), or the flow reads a secret: a run that only took the other side of an
//! `if` asks nothing. (Whether an absence began can itself turn on a secret, so a flow that reads
//! one is always asked.)

use crate::recorder::{NodeTiming, Skipped};
use crate::taint::{SecretFed, Taint};
use crate::wire::SValue;
use holon_core::absence::{self, Level, Look, Seen, Truth};
use holon_core::{capabilities_of, Capability, Resolver, Type, Unit, UnitBody, CALLS};
use std::collections::HashMap;

/// The effects `root`'s run held and didn't make, by its steps (`timeline`, as its `Timer` kept
/// them, before anything of them is withheld) and the reads it made, with the effects the host
/// refused (each by its rule): every level but the flow's own condition, in one order. `walk`
/// false asks no step (no absence began, and no secret is read): only the refusals.
pub(crate) fn of(root: &Unit, resolver: &impl Resolver, timeline: &[NodeTiming], fed: &SecretFed, reads: &[crate::collections::Read], refused: &[(Capability, &str)], walk: bool) -> Vec<Skipped> {
    let mut out: Vec<Skipped> = refused.iter().map(|(cap, rule)| said_refused(cap, rule, fed)).collect();
    if walk && let UnitBody::Composite { children, .. } = &root.body {
        let last = timeline.iter().map(|t| t.round).max().unwrap_or(0);
        let steps = Steps { at: timeline.iter().filter(|t| t.round == last).map(|t| (t.path.as_slice(), t)).collect(), fed, root, resolver };
        // Each step of the root's (an endpoint's handler, a clock's, an action's): each of its
        // outputs that is an effect, or a call it keeps.
        for (k, hash) in children.iter().enumerate() {
            let Some(child) = resolver.resolve(hash) else { continue };
            let at = [k as u32];
            for port in 0..child.outputs.len() as u32 {
                // One made is the host's to say (performed, or refused above), one withheld
                // included: whether it was made is read here only to leave it to the host, and
                // the host's note for it reads as the walk's would.
                if steps.made(&at, port) {
                    continue;
                }
                let Some((leaf, effect)) = effect_of(root, resolver, &at, port) else { continue };
                let Some(origin) = absence::origin(root, resolver, &steps, &at, port) else { continue };
                if !origin.level.kept() {
                    continue;
                }
                out.push(if origin.level == Level::Withheld {
                    withheld(effect)
                } else {
                    let read = reads.iter().position(|r| r.path == origin.at);
                    Skipped { effect, level: origin.level.word().into(), at: leaf, from: origin.at, leaf: origin.leaf, read, unseen: origin.unseen, rule: String::new() }
                });
            }
        }
    }
    out.sort_by(|a, b| key(a).cmp(&key(b)));
    out
}

/// A note's place in the list: by what it says, so it can't turn on which of two things happened.
fn key(s: &Skipped) -> (&str, &str, &[u32], &[u32], &str, Option<usize>, bool, &str) {
    (&s.effect, &s.level, &s.at, &s.from, &s.leaf, s.read, s.unseen, &s.rule)
}

/// An effect that may carry something computed from a secret, not made or refused: the same note.
fn withheld(effect: String) -> Skipped {
    Skipped { effect, level: Level::Withheld.word().into(), ..Skipped::default() }
}

/// A refused effect, by the rule that refused it; one that may carry something computed from a
/// secret said as its absence would be, withheld, whichever rule refused it.
fn said_refused(cap: &Capability, rule: &str, fed: &SecretFed) -> Skipped {
    let effect = format!("{cap:?}");
    if fed.derives_effect(cap) {
        return withheld(effect);
    }
    Skipped { effect, level: Level::Refused.word().into(), rule: rule.into(), ..Skipped::default() }
}

/// The step output `port` of the step at `at` comes from, and the effect it makes, as
/// `performed` names one: none for an output that isn't an effect or a call (an answer).
fn effect_of(root: &Unit, resolver: &impl Resolver, at: &[u32], port: u32) -> Option<(Vec<u32>, String)> {
    let mut path = at.to_vec();
    let mut port = port;
    let mut unit = unit_at(root, resolver, &path)?;
    while let UnitBody::Composite { children, edges } = &unit.body {
        let e = edges.iter().find(|e| e.to_child == holon_core::BOUNDARY && e.to_port == port)?;
        if e.from_child == holon_core::BOUNDARY {
            return None;
        }
        path.push(e.from_child);
        port = e.from_port;
        unit = resolver.resolve(children.get(e.from_child as usize)?)?;
    }
    let UnitBody::Primitive { name } = &unit.body else { return None };
    let effect = unit.outputs.get(port as usize).is_some_and(|p| p.ty == Type::Effect);
    if !effect && !CALLS.contains(&name.as_str()) {
        return None;
    }
    let cap = capabilities_of(name, &unit.params)?.iter().find(|c| !c.is_sourced())?;
    Some((path, format!("{cap:?}")))
}

fn unit_at<'u>(root: &'u Unit, resolver: &'u impl Resolver, path: &[u32]) -> Option<&'u Unit> {
    let mut unit = root;
    for &i in path {
        let UnitBody::Composite { children, .. } = &unit.body else { return None };
        unit = resolver.resolve(children.get(i as usize)?)?;
    }
    Some(unit)
}

/// A run's steps as its `Timer` kept them (its last round's), for the walk.
struct Steps<'a, R> {
    at: HashMap<&'a [u32], &'a NodeTiming>,
    fed: &'a SecretFed,
    root: &'a Unit,
    resolver: &'a R,
}

impl<R: Resolver> Steps<'_, R> {
    /// Whether what the step gives, or whether it gives anything, may turn on a secret: computed
    /// from one, or a read of the flow's own data keyed by one (a collection's item, a file).
    fn withheld(&self, path: &[u32]) -> bool {
        self.fed.derives(path)
            || (self.fed.fed(path) > Taint::Clean
                && unit_at(self.root, self.resolver, path).is_some_and(|u| matches!(&u.body, UnitBody::Primitive { name } if name.starts_with("collection_") || name.starts_with("file_"))))
    }

    /// Whether the root's step at `at` gave output `port` (an effect made, to the host), read
    /// past what is withheld: a step's outputs the record doesn't keep were too large to, so
    /// they were there.
    fn made(&self, at: &[u32], port: u32) -> bool {
        self.at.get(at).is_some_and(|t| t.outputs.as_ref().is_none_or(|o| matches!(o.get(port as usize), Some(Some(_)))))
    }
}

impl<R: Resolver> Seen for Steps<'_, R> {
    fn gave(&self, path: &[u32], port: u32) -> Look {
        if self.withheld(path) {
            return Look::Withheld;
        }
        match self.at.get(path).and_then(|t| t.outputs.as_ref()) {
            None => Look::Unseen,
            Some(o) if matches!(o.get(port as usize), Some(None)) => Look::Nothing,
            Some(_) => Look::Something,
        }
    }
    fn truth(&self, path: &[u32], port: u32) -> Truth {
        if self.withheld(path) {
            return Truth::Withheld;
        }
        match self.at.get(path).and_then(|t| t.outputs.as_ref()) {
            None => Truth::Unseen,
            Some(o) if matches!(o.get(port as usize), Some(Some(SValue::Boolean(true)))) => Truth::True,
            Some(_) => Truth::NotTrue,
        }
    }
    fn said_why(&self, path: &[u32]) -> bool {
        self.at.get(path).is_some_and(|t| t.reason.is_some())
    }
}
