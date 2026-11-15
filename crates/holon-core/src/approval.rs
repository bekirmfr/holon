//! An approval's steps: who decides it, and in what order (docs/L4s/access-and-approvals-L4.md
//! §8.1; docs/L4s/approval-steps-plan.md §2). Stages follow one another; the slots of a stage are
//! filled in any order, each by as many different principals as it says, of its kind, holding its
//! role (none: anyone holding `run.decide` there, the short form `people: N`).
//!
//! The `approval` leaf keeps its steps as one canonical text, which its capability names too
//! (`Approval("payout/person*2")`): stages joined by `>`, a stage's slots by `+`, a slot its kind,
//! `:role` when it has one, `*N` when more than one decides it, and `^` when whoever started the
//! run may take it. This module alone reads and writes that text, so every change to the steps is
//! a change to the text, and to what a person approves.

/// The most slots an approval has, across its stages.
pub const MAX_SLOTS: usize = 5;
/// The most different principals one slot waits for.
pub const MAX_PEOPLE: u8 = 5;
/// The longest role name the steps take: an approval's own name's rule (`kind::ident`).
const MAX_ROLE: usize = 64;

/// Who fills a slot. A service never does (§8.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SlotKind {
    Person,
    Agent,
}

impl SlotKind {
    /// As the canonical text and the language write it.
    pub fn word(self) -> &'static str {
        match self {
            SlotKind::Person => "person",
            SlotKind::Agent => "agent",
        }
    }
}

/// One step of an approval.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Slot {
    pub kind: SlotKind,
    /// The role its principals hold where the approval is; none: anyone holding `run.decide`
    /// there (a person's slot only).
    pub role: Option<String>,
    /// How many different principals fill it, 1 to [`MAX_PEOPLE`].
    pub people: u8,
    /// Whether whoever started the run, or one party with them (an agent they own), may take it
    /// (`starter: true`): an output review, where the one who asked checks what was made, or their
    /// own agent drafts it. Left out, they may not: a request check, where whoever asks is never
    /// the one who says yes (the design's §8.4). An agent's step may say it too; an agent that
    /// started the run never decides it, whatever its steps say.
    pub starter: bool,
}

/// An approval's steps: stages in order, each a set of slots filled in any order.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Steps {
    pub stages: Vec<Vec<Slot>>,
}

impl Steps {
    /// `n` different people, any holding `run.decide`: `{ people: n }`, and an approval with no
    /// options for 1.
    pub fn people(n: u8) -> Steps {
        Steps { stages: vec![vec![Slot { kind: SlotKind::Person, role: None, people: n, starter: false }]] }
    }

    /// The steps `text` writes, when it is canonical: `text()` of what comes back is `text`.
    /// None for anything else, so a hand-written param that isn't fails closed.
    pub fn parse(text: &str) -> Option<Steps> {
        let stages = text.split('>').map(|stage| stage.split('+').map(slot).collect::<Option<Vec<_>>>()).collect::<Option<Vec<_>>>()?;
        let steps = Steps { stages };
        (steps.problem().is_none() && steps.text() == text).then_some(steps)
    }

    /// The canonical text: `Steps::parse(&s.text()) == Some(s)` for steps without a problem.
    pub fn text(&self) -> String {
        let slot = |s: &Slot| {
            let mut t = s.kind.word().to_string();
            if let Some(role) = &s.role {
                t.push(':');
                t.push_str(role);
            }
            if s.people > 1 {
                t.push_str(&format!("*{}", s.people));
            }
            if s.starter {
                t.push('^');
            }
            t
        };
        self.stages.iter().map(|stage| stage.iter().map(slot).collect::<Vec<_>>().join("+")).collect::<Vec<_>>().join(">")
    }

    /// Who decides it, in words: "2 people", "an agent holding classifier, then a person holding
    /// reviewer", "a person holding editor, who may be whoever started the run, or acting for
    /// them".
    pub fn said(&self) -> String {
        let slot = |s: &Slot| {
            let who = match (s.kind, s.people) {
                (SlotKind::Person, 1) => "a person".to_string(),
                (SlotKind::Person, n) => format!("{n} people"),
                (SlotKind::Agent, 1) => "an agent".to_string(),
                (SlotKind::Agent, n) => format!("{n} agents"),
            };
            let who = match &s.role {
                Some(role) => format!("{who} holding {role}"),
                None => who,
            };
            match (s.starter, s.people) {
                (false, _) => who,
                (true, 1) => format!("{who}, who may be whoever started the run, or acting for them"),
                (true, _) => format!("{who}, who may include whoever started the run, or those acting for them"),
            }
        };
        let stage = |slots: &Vec<Slot>| match slots.as_slice() {
            [one] => slot(one),
            [rest @ .., last] => format!("{} and {}, in any order", rest.iter().map(slot).collect::<Vec<_>>().join(", "), slot(last)),
            [] => String::new(),
        };
        self.stages.iter().map(stage).collect::<Vec<_>>().join(", then ")
    }

    /// How many people decide it, when it is the short form: one slot, a person's, with no role,
    /// which whoever started the run doesn't take.
    pub fn short(&self) -> Option<u8> {
        match self.stages.as_slice() {
            [stage] => match stage.as_slice() {
                [Slot { kind: SlotKind::Person, role: None, people, starter: false }] => Some(*people),
                _ => None,
            },
            _ => None,
        }
    }

    /// Each slot with its number (from 0, in the order written) and its stage's.
    pub fn slots(&self) -> impl Iterator<Item = (usize, usize, &Slot)> {
        self.stages.iter().enumerate().flat_map(|(stage, slots)| slots.iter().map(move |s| (stage, s))).enumerate().map(|(n, (stage, s))| (n, stage, s))
    }

    /// Why these steps can't be decided, if they can't: what the language says of them.
    pub fn problem(&self) -> Option<String> {
        let count = self.slots().count();
        if count == 0 || self.stages.iter().any(Vec::is_empty) {
            return Some("an approval has at least one step, and each stage one".into());
        }
        if count > MAX_SLOTS {
            return Some(format!("an approval has at most {MAX_SLOTS} steps, not {count}"));
        }
        for (_, _, s) in self.slots() {
            if !(1..=MAX_PEOPLE).contains(&s.people) {
                return Some(format!("a step is decided by 1 to {MAX_PEOPLE} different principals, not {}", s.people));
            }
            match &s.role {
                Some(role) if !is_role(role) => return Some(format!("{role:?} isn't a role's name: letters, digits, - and _, at most {MAX_ROLE}")),
                None if s.kind == SlotKind::Agent => return Some("an agent's step names the role it decides by: { role: \"classifier\", kind: \"agent\" }".into()),
                _ => {}
            }
        }
        None
    }
}

/// One slot's canonical text: `person`, `agent:classifier`, `person:approver*2`, `person:editor^`.
fn slot(text: &str) -> Option<Slot> {
    let (text, starter) = match text.strip_suffix('^') {
        Some(text) => (text, true),
        None => (text, false),
    };
    let (head, people) = match text.split_once('*') {
        Some((head, n)) => (head, n.parse::<u8>().ok()?),
        None => (text, 1),
    };
    let (kind, role) = match head.split_once(':') {
        Some((kind, role)) => (kind, Some(role.to_string())),
        None => (head, None),
    };
    let kind = match kind {
        "person" => SlotKind::Person,
        "agent" => SlotKind::Agent,
        _ => return None,
    };
    Some(Slot { kind, role, people, starter })
}

/// A role's name as the steps take it: letters, digits, `-` and `_`, at most 64. A role an admin
/// defines is held to it too, so every role can be named in an approval (D35).
pub fn is_role(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_ROLE && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Steps enough to try every rule on, a few hundred: every slot variant (a person with no role or
/// a role, an agent with a role, either whoever started the run may take or not; 1, 2 or 5 of
/// them) in every shape of stages (one to three, of one to three slots, five at most), from each
/// starting variant. For the round trips' tests, here and in the language's.
#[doc(hidden)]
pub fn universe() -> Vec<Steps> {
    let mut variants = Vec::new();
    let person = SlotKind::Person;
    for (kind, role, starter) in [
        (person, None, false),
        (person, Some("reviewer"), false),
        (person, Some("x-1"), false),
        (person, None, true),
        (person, Some("reviewer"), true),
        (SlotKind::Agent, Some("reviewer"), false),
        (SlotKind::Agent, Some("x-1"), false),
        (SlotKind::Agent, Some("reviewer"), true),
    ] {
        for people in [1, 2, 5] {
            variants.push(Slot { kind, role: role.map(str::to_string), people, starter });
        }
    }
    let mut shapes: Vec<Vec<usize>> = Vec::new();
    for stages in 1..=3usize {
        let mut shape = vec![1; stages];
        loop {
            if shape.iter().sum::<usize>() <= MAX_SLOTS {
                shapes.push(shape.clone());
            }
            // The next shape of this many stages, each of 1 to 3 slots.
            let Some(at) = shape.iter().rposition(|&n| n < 3) else { break };
            shape[at] += 1;
            shape[at + 1..].iter_mut().for_each(|n| *n = 1);
        }
    }
    let mut all = Vec::new();
    for start in 0..variants.len() {
        for shape in &shapes {
            let mut next = start;
            let stages = shape
                .iter()
                .map(|&n| {
                    (0..n)
                        .map(|_| {
                            next += 1;
                            variants[(next - 1) % variants.len()].clone()
                        })
                        .collect()
                })
                .collect();
            all.push(Steps { stages });
        }
    }
    all
}

/// Why a flow's waits can't each be asked, if one can't: two named alike, two approvals asking
/// for other steps, or an approval and an event (`consent`, its permissions:
/// `Approval("payout/person")`, `Await("went")`). A run keeps one decision or delivery a name, so
/// the second would be answered with the first's, and what it asks for, which a review shows,
/// never asked. The language refuses the second where it is written; a server, a unit built
/// otherwise (a canvas).
pub fn named_apart<'a>(consent: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut seen: std::collections::BTreeMap<&str, Option<&str>> = std::collections::BTreeMap::new();
    for item in consent {
        let (name, steps) = if let Some(rest) = item.strip_prefix("Approval(\"").and_then(|r| r.strip_suffix("\")")) {
            let Some((name, steps)) = rest.split_once('/') else { continue };
            (name, Some(steps))
        } else if let Some(name) = item.strip_prefix("Await(\"").and_then(|r| r.strip_suffix("\")")) {
            (name, None)
        } else {
            continue;
        };
        match seen.get(name) {
            Some(was) if *was != steps => return Some(apart(name, *was, steps)),
            Some(_) => {}
            None => {
                seen.insert(name, steps);
            }
        }
    }
    None
}

/// What a flow is told of two waits named `name` alike, one waiting for `a` and the other for `b`:
/// an approval's steps (its canonical text), or none, an event.
pub fn apart(name: &str, a: Option<&str>, b: Option<&str>) -> String {
    let what = |s: Option<&str>| match s {
        Some(s) => format!("an approval asking for {}", Steps::parse(s).map_or_else(|| s.to_string(), |s| s.said())),
        None => "an event".to_string(),
    };
    format!("{} and {} are both named {name:?}: a run keeps one decision a name, so the second would be answered with the first's, and what it asks for never asked. Name them apart", what(a), what(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// **Guards:** the canonical text is one text for each approval and back: written and read, any
    /// steps come back as they were, and no two steps share a text.
    /// **Why:** the text is the capability a person approves (`Approval("payout/…")`): two steps
    /// written the same would let a version weaken one without asking, and a text read back
    /// otherwise would decide by other steps than the ones approved.
    #[test]
    fn every_steps_text_reads_back_as_the_steps_and_no_two_share_one() {
        let all = universe();
        assert!((500..600).contains(&all.len()), "{}", all.len());
        let mut texts = BTreeSet::new();
        for steps in &all {
            assert_eq!(steps.problem(), None, "{steps:?}");
            let text = steps.text();
            assert_eq!(Steps::parse(&text).as_ref(), Some(steps), "{text}");
            assert!(!steps.said().is_empty());
            texts.insert(text);
        }
        assert_eq!(texts.len(), all.iter().collect::<BTreeSet<_>>().len(), "one text each");
        let shapes: BTreeSet<Vec<usize>> = all.iter().map(|s| s.stages.iter().map(Vec::len).collect()).collect();
        assert_eq!(shapes.len(), 21, "every shape: {shapes:?}");
    }

    #[test]
    fn a_text_that_isnt_canonical_is_no_steps() {
        let long = format!("person:{}", "r".repeat(65));
        for text in [
            "", "person*1", "person*02", "person*6", "person*0", "person:", "agent", "agent*2", "service:x", "Person", "person:a:b", "person*2*2", ">person",
            "person>", "person++person", "person >person", "person:a b", "person+person+person+person+person+person", &long,
            // whoever started the run: once, and last
            "person^^", "person^*2", "^person", "person:a^b", "agent:a^^", "agent^", "agent:a^*2", "person^ ",
        ] {
            assert_eq!(Steps::parse(text), None, "{text:?}");
        }
        assert!(Steps::parse("agent:a^").is_some_and(|s| s.slots().all(|(_, _, s)| s.starter && s.kind == SlotKind::Agent)), "an agent's step may take whoever started the run's party");
        assert_eq!(Steps::parse("person"), Some(Steps::people(1)));
        assert_eq!(Steps::parse("person*2").and_then(|s| s.short()), Some(2));
        assert_eq!(Steps::parse("person:approver*2").and_then(|s| s.short()), None, "a role is the long form");
        assert_eq!(Steps::parse("person^").and_then(|s| s.short()), None, "so is a step whoever started the run may take");
        assert!(Steps::parse("person:editor*2^").is_some_and(|s| s.slots().all(|(_, _, s)| s.starter && s.people == 2)));
    }

    #[test]
    fn the_steps_say_who_decides_and_in_what_order() {
        let said = |text: &str| Steps::parse(text).unwrap().said();
        assert_eq!(said("person"), "a person");
        assert_eq!(said("person*2"), "2 people");
        assert_eq!(said("agent:classifier>person:reviewer>person:approver"), "an agent holding classifier, then a person holding reviewer, then a person holding approver");
        assert_eq!(said("person:a+person:b>person:c*2"), "a person holding a and a person holding b, in any order, then 2 people holding c");
        assert_eq!(said("agent:a*2+person+person:b"), "2 agents holding a, a person and a person holding b, in any order");
        assert_eq!(said("agent:drafter>person:editor^"), "an agent holding drafter, then a person holding editor, who may be whoever started the run, or acting for them");
        assert_eq!(said("person*2^"), "2 people, who may include whoever started the run, or those acting for them");
        assert_eq!(said("agent:drafter^>person:editor^"), "an agent holding drafter, who may be whoever started the run, or acting for them, then a person holding editor, who may be whoever started the run, or acting for them");
        let numbered: Vec<(usize, usize)> = Steps::parse("person:a+person:b>person:c").unwrap().slots().map(|(n, stage, _)| (n, stage)).collect();
        assert_eq!(numbered, [(0, 0), (1, 0), (2, 1)]);
    }
}
