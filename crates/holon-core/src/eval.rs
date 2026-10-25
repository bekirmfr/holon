//! Graph evaluator (P1.4.3), lazy at its choices (docs/L4s/lazy-choices-L4.md).
//!
//! `eval(unit, inputs, resolver) -> EvalOutputs` (`SmallVec<[Option<Value>; 1]>`, inline for the
//! common 0-1-output case — the SmallVec perf inch, 2026-07-02). A primitive dispatches to its
//! native `compute`; a composite resolves its inner DAG **by reference** (I3), injecting promoted
//! inputs through `BOUNDARY→child` edges and reading each exposed output through its own
//! `child→BOUNDARY` edge (P4.7: a composite may expose several; a primitive always exposes
//! exactly one — multi-output is composite-only). A primitive and a composite are evaluated
//! through the **identical** entry point (I1). Each output slot independently `None`s ("not
//! computable") and **propagates** — one failed slot never blanks out the others.
//!
//! ## Which nodes run
//! A composite is evaluated from its roots (the children wired to its outputs, and the children
//! nothing consumes) back through what each node needs: every wired input for an ordinary leaf,
//! and for a choice only what [`choice_next`] says: `guard` its value only when its condition is
//! `true`, `coalesce` its second input only when its first is absent. A node only a choice that
//! didn't need it consumes is not run, so a branch not taken asks the host nothing. Every leaf is
//! pure and total, so the values are the eager evaluator's (`eager.rs`, the tests' oracle).
//!
//! ## Pending
//! Within a run's round, a request the host hasn't answered yet, or a waiting point whose event
//! hasn't come, is [`Slot::Pending`], not absent: "not yet", where absent is "nothing will come".
//! A node that needs a pending input is pending, without being run, and a choice whose deciding
//! input is pending waits instead of deciding. At the public edge pending reads as `None`.
//!
//! ## Higher-order `map`
//! `compute` stays purely first-order. `map` is higher-order — it resolves a referenced
//! unit and applies it per element — so it is handled **here**, where recursion already
//! lives, rather than via a callback into a first-order `compute`. Its `fn` argument is a
//! param holding the hex hash of the unit to apply.

use crate::{primitives, Edge, Hash, Param, Resolver, Unit, UnitBody, Value, BOUNDARY, VARIADIC};
use crate::hash::parse_hash;
use smallvec::{smallvec, SmallVec};
use std::collections::{BTreeMap, BTreeSet};

/// The evaluator's own output shape (P4.7 + the SmallVec perf inch, 2026-07-02).
/// Inline storage for 0 or 1 outputs — every genesis primitive declares exactly one, forever,
/// and a single-output composite is still the overwhelming majority even now that multi-output
/// is a real, everyday editor capability — so the common case never touches the heap. Spills to
/// the heap only for a genuine multi-output composite, behaving exactly as a plain `Vec` would.
pub type EvalOutputs = SmallVec<[Option<Value>; 1]>;

/// What one output is, as the evaluator knows it in this round.
#[derive(Clone, Debug, PartialEq)]
pub enum Slot {
    Value(Value),
    /// Nothing will come: what `None` means at the public edge.
    Absent,
    /// Not yet: it waits on an answer the host hasn't given in this round.
    Pending,
}

impl Slot {
    fn of(v: Option<Value>) -> Slot {
        v.map_or(Slot::Absent, Slot::Value)
    }
    /// The value, if there is one now (pending reads as none).
    pub fn value(&self) -> Option<&Value> {
        match self {
            Slot::Value(v) => Some(v),
            _ => None,
        }
    }
    fn into_value(self) -> Option<Value> {
        match self {
            Slot::Value(v) => Some(v),
            _ => None,
        }
    }
}

/// One slot per output of a node, inline for the common one.
type Slots = SmallVec<[Slot; 1]>;

/// The host's answer to what a node asks: a value, nothing (an item the collection doesn't have,
/// a request a replay's record lacks), or not yet (a request still to be performed, an event that
/// hasn't come).
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Value(Value),
    Absent,
    NotYet,
}

impl From<Option<Value>> for Answer {
    fn from(v: Option<Value>) -> Answer {
        v.map_or(Answer::Absent, Answer::Value)
    }
}

impl From<Answer> for Slot {
    fn from(a: Answer) -> Slot {
        match a {
            Answer::Value(v) => Slot::Value(v),
            Answer::Absent => Slot::Absent,
            Answer::NotYet => Slot::Pending,
        }
    }
}

/// Evaluate a unit against values for its input ports (index = input-port index;
/// `None` = absent). Returns one slot per the unit's declared output — `unit.outputs.len()`
/// entries, always, in order; each slot independently `Some`/`None` (P4.7: multi-output is a
/// composite-only capability — every primitive declares exactly one output, so its arm always
/// returns a 1-element vec; only a composite's own boundary can expose several).
pub fn eval(unit: &Unit, inputs: &[Option<Value>], resolver: &impl Resolver) -> EvalOutputs {
    eval_observed(unit, inputs, resolver, &mut NoObserver)
}

/// The host's window into a run. Told when each composite child starts and finishes evaluating,
/// by its path of child indices from the root (`[2]`, then `[2, 0]` inside it, …), and asked for
/// the answer to each `http_request` whose request is ready. Core never reads a clock or does IO:
/// an observer is how the host times each step (the flow log's timeline) and answers requests
/// (the rounds of §4.1) without that impurity entering here. A child that isn't run (a branch
/// not taken, or one waiting on a pending input) is neither entered nor asked. Inside a
/// `map`/`fold`, each application of the function is at the node's path then the item's index
/// (`[2, 0]` for the first item of the `map` at `[2]`, `[2, 0, 1]` for that application's child
/// 1, …): its requests and reads are asked for there, each item's its own, but its steps aren't
/// reported, so a path one below a node is still that node's own child.
pub trait Observer {
    fn enter(&mut self, _path: &[u32]) {}
    fn leave(&mut self, _path: &[u32]) {}
    /// What the node at `path` produced, called just before its `leave`. A host may keep it (the
    /// flow log does), so a replay can show what a node it can't recompute, one fed a secret it
    /// never logs, actually produced. A pending slot is one still waiting on an answer.
    fn produced(&mut self, _path: &[u32], _outputs: &[Slot]) {}
    /// The answer to `request` (`{method, url, headers, body}`, a collection read, a waiting
    /// point), asked by the node at `path`. [`Answer::Absent`] (the default) leaves the node's
    /// output absent; [`Answer::NotYet`] leaves it pending: in a round, the host performs the
    /// request before the next one.
    fn answer(&mut self, _path: &[u32], _request: &Value) -> Answer {
        Answer::Absent
    }
    /// Round `n` is about to be evaluated. Core never calls this; the host's round loop does,
    /// so an observer can tell one round's steps from the next.
    fn round(&mut self, _n: usize) {}
    /// The module at `path` gave nothing, and this is why (`Resolver::run_module`'s error: out
    /// of fuel, a trap, a body's exception as the engine words it). Called just before its
    /// `produced`, so a host can keep the reason beside the node for a replay to show.
    fn failed(&mut self, _path: &[u32], _reason: &str) {}
    /// What the node at `path` (the unit `unit`) produced, when the observer stands that in for
    /// what core would compute from `inputs`: none (the default) computes the node. A check that
    /// runs a recorded run again (`holon verify`) answers for a node whose inputs carry what the
    /// record never holds (a secret's placeholder) with the outputs the record kept for it, and
    /// checks the rest of the run against them. An answer with the wrong number of outputs is
    /// ignored, and the node computed.
    fn asserted(&mut self, _path: &[u32], _unit: &Unit, _inputs: &[Option<Value>]) -> Option<Vec<Option<Value>>> {
        None
    }
}

/// Observes nothing: what plain [`eval`] uses.
pub struct NoObserver;
impl Observer for NoObserver {}

/// [`eval`], reporting each composite child's evaluation to `observer`. The result is identical.
pub fn eval_observed(
    unit: &Unit,
    inputs: &[Option<Value>],
    resolver: &impl Resolver,
    observer: &mut impl Observer,
) -> EvalOutputs {
    eval_at(unit, inputs, resolver, observer, &mut Vec::new()).into_iter().map(Slot::into_value).collect()
}

/// What the host answers to a node's request, or absent when the request can't even be built.
fn ask(request: Option<Value>, observer: &mut impl Observer, path: &[u32]) -> Slots {
    smallvec![request.map_or(Slot::Absent, |r| observer.answer(path, &r).into())]
}

fn eval_at(
    unit: &Unit,
    inputs: &[Option<Value>],
    resolver: &impl Resolver,
    observer: &mut impl Observer,
    path: &mut Vec<u32>,
) -> Slots {
    match &unit.body {
        // `map` is higher-order — it invokes the evaluator — so it lives here, where
        // recursion lives, rather than in first-order `compute`.
        UnitBody::Primitive { name } if name == "map" => smallvec![eval_map(inputs, &unit.params, resolver, observer, path)],
        // `fold` is the one iteration primitive (the universal list donor): also higher-order,
        // so it lives here too. It threads an accumulator, which `map` does not.
        UnitBody::Primitive { name } if name == "fold" => smallvec![eval_fold(inputs, &unit.params, resolver, observer, path)],
        // An outbound request: built here, answered (or not yet) by the host through the observer.
        UnitBody::Primitive { name } if name == "http_request" => ask(primitives::http_request(inputs, &unit.params), observer, path),
        // One whose body is a file: the host streams it.
        UnitBody::Primitive { name } if name == "http_upload" => ask(primitives::http_upload(inputs, &unit.params), observer, path),
        // One whose answer the host forwards to the route's caller as it arrives.
        UnitBody::Primitive { name } if name == "http_stream" => ask(primitives::http_stream(inputs, &unit.params), observer, path),
        // A durable run's waiting point and its token: also the host's to answer.
        UnitBody::Primitive { name } if name == "await" => ask(primitives::await_request(&unit.params), observer, path),
        // A collection read: the host answers it from the collection as the run found it.
        UnitBody::Primitive { name } if name == "collection_get" => ask(primitives::collection_get_request(inputs, &unit.params), observer, path),
        UnitBody::Primitive { name } if name == "collection_list" => ask(primitives::collection_list_request(inputs, &unit.params), observer, path),
        // A file read: the host answers it from the folder bound to the name, as the run found it.
        UnitBody::Primitive { name } if name == "file_read" => ask(primitives::file_read_request(inputs, &unit.params), observer, path),
        UnitBody::Primitive { name } if name == "file_list" => ask(primitives::file_list_request(inputs, &unit.params), observer, path),
        UnitBody::Primitive { name } if name == "file_handle" => ask(primitives::file_handle_request(inputs, &unit.params), observer, path),
        UnitBody::Primitive { name } if name == "file_lines" => ask(primitives::file_lines_request(inputs, &unit.params), observer, path),
        UnitBody::Primitive { name } if name == "file_slice" => ask(primitives::file_slice_request(inputs, &unit.params), observer, path),
        UnitBody::Primitive { name } if name == "resume_token" => ask(primitives::resume_token_request(&unit.params), observer, path),
        // `coalesce` is the one leaf whose meaning is about absence: the first input that
        // arrived. The generic arm below would give the same answer today (it gathers present
        // inputs in port order), but that is a property of its implementation, not its
        // contract, and the firing rule it serves says the opposite ("a wired input absent,
        // nothing fires"). This arm is the contract, stated once, where absence is visible.
        // (Inside a composite it is a choice, decided by `choice_next` without getting here.)
        UnitBody::Primitive { name } if name == "coalesce" => smallvec![Slot::of(inputs.iter().flatten().next().cloned())],
        UnitBody::Primitive { name } => {
            // Gather present values in port order **by reference** — `compute` borrows them,
            // so an O(1) reader (head/length/is_empty) copies only what it returns, not the
            // whole input list. compute's arity check is still the firing rule.
            //
            // That rule alone fails *open* for a variadic leaf: dropping an absent input leaves
            // a shorter list it still accepts, so `and(authenticated, <absent>)` would yield
            // `authenticated` and `list(a, <absent>, c)` would yield `[a, c]`. A variadic leaf
            // therefore fires only when every declared input is present, exactly as a
            // fixed-arity leaf already does.
            if VARIADIC.contains(&name.as_str()) && inputs.iter().any(Option::is_none) {
                return smallvec![Slot::Absent];
            }
            let present: Vec<&Value> = inputs.iter().flatten().collect();
            smallvec![Slot::of(primitives::compute(name, &present, &unit.params))]
        }
        UnitBody::Composite { children, edges } => {
            eval_composite(children, edges, unit.outputs.len(), inputs, resolver, observer, path)
        }
        // A module body: the engine behind the resolver runs it (docs/L4s/wasm-module-L4.md §5);
        // when it can't, the observer hears why, at this node.
        UnitBody::Module { bytes } => {
            let (outs, reason) = run_module_outputs(bytes, unit, inputs, resolver);
            if let Some(reason) = reason {
                observer.failed(path, &reason);
            }
            outs.into_iter().map(Slot::of).collect()
        }
    }
}

/// A module's outputs, one per declared port: through `Resolver::run_module`, under the budgets
/// its params give. Strict like any leaf: an absent input or a bad budget makes every output
/// absent, silently (as any leaf's missing input would); the engine's refusal (no engine, a
/// trap, a budget run out, a body's error) makes them absent with the reason. Shared with the
/// eager oracle (`eager.rs`).
pub(crate) fn run_module_outputs(bytes: &[u8], unit: &Unit, inputs: &[Option<Value>], resolver: &impl Resolver) -> (Vec<Option<Value>>, Option<String>) {
    let n = unit.outputs.len();
    if inputs.len() != unit.inputs.len() || inputs.iter().any(Option::is_none) {
        return (vec![None; n], None);
    }
    let limits = match crate::module::module_limits(&unit.params) {
        Ok(l) => l,
        Err(e) => return (vec![None; n], Some(e.to_string())),
    };
    match resolver.run_module(bytes, &limits, inputs) {
        Ok(outs) => ((0..n).map(|i| outs.get(i).cloned().flatten()).collect(), None),
        Err(reason) => (vec![None; n], Some(reason)),
    }
}

/// Convenience for the (still overwhelming majority of) callers that only ever deal with a
/// single-output unit: the one output, or `None` if the unit doesn't declare *exactly* one —
/// zero outputs, or two-or-more, both fail closed here rather than silently taking the first of
/// several. `map`/`fold` apply their `fn` by the same rule (`apply`, below): a multi-output
/// combinator isn't yet a meaningful thing (would it build a list of tuples? a tuple of lists?
/// — a real, separate design question, deferred per the P4.7 L4 §10), so refusing it here is
/// honest, not a workaround.
pub fn eval_one(unit: &Unit, inputs: &[Option<Value>], resolver: &impl Resolver) -> Option<Value> {
    let mut outs = eval(unit, inputs, resolver);
    if outs.len() == 1 {
        outs.pop().unwrap()
    } else {
        None
    }
}

/// What a choice needs next, given what it has seen of its inputs.
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    /// Evaluate the input at this port, then ask again.
    Need(usize),
    /// Decided: its output, without the inputs it didn't need.
    Done(Slot),
}

/// The choice leaves' rule: what `guard` or `coalesce` needs next, or its output, given the
/// inputs seen so far (`seen[p]`: `None` not yet evaluated, else its slot). `None` for a leaf
/// that isn't a choice. `guard` evaluates its value only when its condition is `true`;
/// `coalesce` its second input only when its first is absent; either waits while its deciding
/// input is pending. Each gives exactly what it gives with every input evaluated. (`select` is
/// strict by meaning, an absent side making it absent, so it needs both sides and isn't one.)
/// The editor's replay applies the same rule through wasm.
pub fn choice_next(name: &str, seen: &[Option<Slot>]) -> Option<Choice> {
    let at = |p: usize| seen.get(p).cloned().unwrap_or(Some(Slot::Absent));
    Some(match name {
        "guard" => match at(0) {
            None => Choice::Need(0),
            Some(Slot::Pending) => Choice::Done(Slot::Pending),
            Some(Slot::Value(Value::Boolean(true))) => match at(1) {
                None => Choice::Need(1),
                Some(v) => Choice::Done(v),
            },
            // false, absent, or not a boolean: absent, as `guard` is.
            Some(_) => Choice::Done(Slot::Absent),
        },
        "coalesce" => match at(0) {
            None => Choice::Need(0),
            Some(Slot::Absent) => match at(1) {
                None => Choice::Need(1),
                Some(v) => Choice::Done(v),
            },
            Some(first) => Choice::Done(first),
        },
        _ => return None,
    })
}

fn eval_composite(
    children: &[Hash],
    edges: &[Edge],
    n_outputs: usize,
    inputs: &[Option<Value>],
    resolver: &impl Resolver,
    observer: &mut impl Observer,
    path: &mut Vec<u32>,
) -> Slots {
    let n = children.len();

    // Resolve children (by hash). Any unresolved ⇒ nothing evaluable — every declared output
    // slot is absent (the vec's *length* is still the declared contract; only the values are
    // absent, mirroring how `inputs` already has a fixed length with individually-absent slots).
    let mut resolved: Vec<&Unit> = Vec::with_capacity(n);
    for h in children {
        match resolver.resolve(h) {
            Some(u) => resolved.push(u),
            None => return smallvec![Slot::Absent; n_outputs],
        }
    }

    // Index incoming edges: destination (to_child, to_port) → source (from_child, from_port).
    // Validation guarantees ≤1 source per destination, so this never loses an edge.
    let mut incoming: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    for e in edges {
        incoming.insert((e.to_child, e.to_port), (e.from_child, e.from_port));
    }

    // A cycle is impossible after P1.3.1; handled, not panicked, and the walk below can then
    // rely on reaching the end of everything it pulls.
    if topo_order(n, edges).is_none() {
        return smallvec![Slot::Absent; n_outputs];
    }

    // The roots: what the outputs read, and what nothing consumes (a node placed on its own is
    // run, as it always was). In ascending child index, so the walk is deterministic.
    let mut consumed = vec![false; n];
    let mut root = vec![false; n];
    for e in edges {
        if (e.from_child as usize) < n {
            consumed[e.from_child as usize] = true;
            if e.to_child == BOUNDARY {
                root[e.from_child as usize] = true;
            }
        }
    }

    // Per-evaluation scratch, indexed by child: `None` not evaluated (yet, or at all), else
    // EACH child's own output slots (a child may itself be a multi-output composite).
    let mut scratch: Vec<Option<Slots>> = vec![None; n];

    // The slot feeding a child's input port: `None` while its source child is unevaluated.
    let slot_in = |scratch: &[Option<Slots>], ci: usize, p: usize| -> Option<Slot> {
        match incoming.get(&(ci as u32, p as u32)) {
            Some(&(BOUNDARY, fp)) => Some(Slot::of(inputs.get(fp as usize).cloned().flatten())),
            // `get`, not indexing: an edge naming a child that does not exist (bytes that never
            // went through `validate`) reads as absent.
            Some(&(fc, fp)) => match scratch.get(fc as usize) {
                Some(Some(outs)) => Some(outs.get(fp as usize).cloned().unwrap_or(Slot::Absent)),
                Some(None) => None,
                None => Some(Slot::Absent),
            },
            None => Some(Slot::Absent),
        }
    };

    // Depth-first from the roots, with an explicit stack: a node is run once everything it
    // needs is evaluated; until then, what it needs next is pushed.
    let mut stack: Vec<usize> = (0..n).rev().filter(|&c| root[c] || !consumed[c]).collect();
    while let Some(&ci) = stack.last() {
        if scratch[ci].is_some() {
            stack.pop();
            continue;
        }
        let child = resolved[ci];
        let k = child.inputs.len();
        let seen: Vec<Option<Slot>> = (0..k).map(|p| slot_in(&scratch, ci, p)).collect();
        let choice = match &child.body {
            UnitBody::Primitive { name } => choice_next(name, &seen),
            UnitBody::Composite { .. } | UnitBody::Module { .. } => None,
        };
        // A choice pulls what it needs; anything else, every input not yet evaluated.
        let need = match &choice {
            Some(Choice::Need(p)) => Some(*p),
            Some(Choice::Done(_)) => None,
            None => seen.iter().position(Option::is_none),
        };
        if let Some(p) = need {
            let &(fc, _) = incoming.get(&(ci as u32, p as u32)).expect("only a wired port is unevaluated");
            stack.push(fc as usize);
            continue;
        }
        stack.pop();
        let outs: Slots = match choice {
            Some(Choice::Done(slot)) => {
                path.push(ci as u32);
                observer.enter(path);
                let outs: Slots = smallvec![slot];
                observer.produced(path, &outs);
                observer.leave(path);
                path.pop();
                outs
            }
            _ => {
                let seen: Vec<Slot> = seen.into_iter().flatten().collect();
                if seen.contains(&Slot::Pending) {
                    // Waiting on an answer: not run in this round.
                    smallvec![Slot::Pending; child.outputs.len()]
                } else {
                    // A leaf with optional ports (an `http_request`'s headers and body) reads an
                    // *unwired* one as "none"; a *wired* one whose value is absent means what
                    // absence means everywhere else: the node doesn't fire. Otherwise a request
                    // downstream of a check that failed (a webhook whose signature didn't
                    // verify) still went out, bodiless.
                    let gathered: Vec<Option<Value>> = seen.into_iter().map(Slot::into_value).collect();
                    let wired_but_absent = (0..k).any(|p| gathered[p].is_none() && incoming.contains_key(&(ci as u32, p as u32)));
                    let held = wired_but_absent && matches!(&child.body, UnitBody::Primitive { name } if primitives::has_optional_ports(name));
                    path.push(ci as u32);
                    observer.enter(path);
                    let outs = if held {
                        smallvec![Slot::Absent; child.outputs.len()]
                    } else {
                        match observer.asserted(path, child, &gathered) {
                            Some(asserted) if asserted.len() == child.outputs.len() => asserted.into_iter().map(Slot::of).collect(),
                            _ => eval_at(child, &gathered, resolver, observer, path),
                        }
                    };
                    observer.produced(path, &outs);
                    observer.leave(path);
                    path.pop();
                    outs
                }
            }
        };
        scratch[ci] = Some(outs);
    }

    // Each exposed output is whatever feeds that boundary-output port: a child's output at the
    // right port, or — symmetric with the child-input case above — a promoted input routed
    // straight through. (A composite whose output *is* one of its inputs is a valid,
    // validate-accepted shape; the BOUNDARY→BOUNDARY arm below is why that doesn't panic.)
    (0..n_outputs as u32)
        .map(|op| match incoming.get(&(BOUNDARY, op)) {
            Some(&(BOUNDARY, fp)) => Slot::of(inputs.get(fp as usize).cloned().flatten()),
            Some(&(fc, fp)) => scratch.get(fc as usize).and_then(|o| o.as_ref()).and_then(|o| o.get(fp as usize)).cloned().unwrap_or(Slot::Absent),
            None => Slot::Absent,
        })
        .collect()
}

/// Kahn's algorithm over in-range child→child edges, emitting a deterministic order
/// (ascending child index among ready nodes). Boundary edges are excluded (an endpoint
/// `== BOUNDARY` is not `< n`). Returns `None` on a cycle — impossible after P1.3.1, but
/// handled rather than panicking.
pub(crate) fn topo_order(n: usize, edges: &[Edge]) -> Option<Vec<usize>> {
    let mut indeg = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in edges {
        let (f, t) = (e.from_child as usize, e.to_child as usize);
        if f < n && t < n {
            adj[f].push(t);
            indeg[t] += 1;
        }
    }
    let mut ready: BTreeSet<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(&node) = ready.iter().next() {
        ready.remove(&node);
        order.push(node);
        for &m in &adj[node] {
            indeg[m] -= 1;
            if indeg[m] == 0 {
                ready.insert(m);
            }
        }
    }
    if order.len() == n {
        Some(order)
    } else {
        None
    }
}

/// Higher-order `map`: input 0 is a list; the `fn` param holds the hex hash of the unit
/// to apply to each element. Yields a list of results, or absent if the list/param is
/// malformed or any application is absent (None-propagation), or pending while any application
/// waits on an answer. The `fn` reference rides a param, so `map`'s hash transitively depends on
/// `fn`'s hash (I6 holds via the preimage); children-based *inspection* (P3.2.1) must
/// additionally treat hash-valued params as references.
///
/// Inputs after the list are passed to every application after the item, the same each time:
/// `fn(elem, extra…)`. That is how a function uses a value from around it (a lambda's capture,
/// lifted to an input of its own) while staying a sealed unit. Like any variadic leaf, `map`
/// fires only when every extra input is present. Each application asks the host for what it
/// requests or reads at its own path (see [`Observer`]).
fn eval_map(inputs: &[Option<Value>], params: &[Param], resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slot {
    let list = match inputs.first() {
        Some(Some(Value::List(xs))) => xs,
        _ => return Slot::Absent,
    };
    let Some(extra) = present(&inputs[1..]) else { return Slot::Absent };
    let Some(unit) = function(params, resolver) else { return Slot::Absent };

    let mut out = Vec::with_capacity(list.len());
    let (mut pending, mut absent) = (false, false);
    let mut args: Vec<Option<Value>> = Vec::with_capacity(1 + extra.len());
    let mut asks = Asks(observer);
    for (k, elem) in list.iter().enumerate() {
        args.clear();
        args.push(Some(elem.clone()));
        args.extend(extra.iter().cloned());
        // Every item is applied, even after one comes out absent, so each item's requests are
        // asked in the same round rather than one round per item.
        match apply(unit, &args, resolver, &mut asks, path, k) {
            Slot::Value(v) => out.push(v),
            Slot::Absent => absent = true,
            Slot::Pending => pending = true,
        }
    }
    if pending {
        Slot::Pending
    } else if absent {
        Slot::Absent
    } else {
        Slot::Value(Value::List(out))
    }
}

/// The unit a `map`/`fold` applies, named by its `fn` param.
fn function<'r>(params: &[Param], resolver: &'r impl Resolver) -> Option<&'r Unit> {
    let fn_hex = params.iter().find(|p| p.name() == "fn").and_then(|p| p.value())?;
    resolver.resolve(&parse_hash(fn_hex)?)
}

/// One application of a `map`/`fold` function, to item `k`: evaluated at the node's path then
/// `k`, its one output (or absent, as [`eval_one`]).
fn apply(unit: &Unit, args: &[Option<Value>], resolver: &impl Resolver, asks: &mut Asks, path: &mut Vec<u32>, k: usize) -> Slot {
    let Ok(k) = u32::try_from(k) else { return Slot::Absent };
    path.push(k);
    let mut outs = eval_at(unit, args, resolver, asks, path);
    path.pop();
    if outs.len() == 1 { outs.pop().unwrap() } else { Slot::Absent }
}

/// The observer inside a `map`/`fold` application: it passes on what is asked (requests, reads,
/// waiting points) and nothing of the steps. One type however deep the nesting, so a function
/// that maps again evaluates under the same observer type.
struct Asks<'a>(&'a mut dyn Observer);

impl Observer for Asks<'_> {
    fn answer(&mut self, path: &[u32], request: &Value) -> Answer {
        self.0.answer(path, request)
    }
}

/// The inputs, when every one is present.
fn present(inputs: &[Option<Value>]) -> Option<&[Option<Value>]> {
    inputs.iter().all(Option::is_some).then_some(inputs)
}

/// Higher-order `fold` — the **one** iteration primitive, and the universal list donor
/// (`sum`/`map`/`filter`/`length`/… are all composites over it). Input 0 is the initial
/// accumulator, input 1 is a list; the `fn` param holds the hex hash of the combining unit,
/// applied left-to-right as `fn(acc, elem)`. Yields the final accumulator; `fold(init, [])`
/// = `init`. Iteration is bounded by the list length — there is no unbounded looping
/// construct anywhere in the vocabulary, so **every composite is total** (the property the
/// trustless cache leans on). Any application returning absent propagates, and one waiting on
/// an answer leaves the fold pending. Like `map`, the `fn` reference rides a param, so `fold`'s
/// hash transitively depends on it (I6), and inspection (P3.2.1) must treat hash-valued params
/// as references. Inputs after the list are passed after the item, as `map` passes them:
/// `fn(acc, elem, extra…)`, and each application asks at its own path, as `map`'s do.
fn eval_fold(inputs: &[Option<Value>], params: &[Param], resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slot {
    let init = match inputs.first() {
        Some(Some(v)) => v.clone(),
        _ => return Slot::Absent,
    };
    let list = match inputs.get(1) {
        Some(Some(Value::List(xs))) => xs,
        _ => return Slot::Absent,
    };
    let Some(extra) = present(inputs.get(2..).unwrap_or_default()) else { return Slot::Absent };
    let Some(unit) = function(params, resolver) else { return Slot::Absent };

    let mut acc = init;
    let mut args: Vec<Option<Value>> = Vec::with_capacity(2 + extra.len());
    let mut asks = Asks(observer);
    for (k, elem) in list.iter().enumerate() {
        // fn(acc, elem, extra…) → new acc; absent or pending ⇒ the whole fold is.
        args.clear();
        args.push(Some(acc));
        args.push(Some(elem.clone()));
        args.extend(extra.iter().cloned());
        acc = match apply(unit, &args, resolver, &mut asks, path, k) {
            Slot::Value(v) => v,
            other => return other,
        };
    }
    Slot::Value(acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MapResolver, Port, Type};
    use std::collections::BTreeMap;

    fn num(n: f64) -> Value { Value::Number(n) }
    fn nport(name: &str) -> Port { Port { name: name.into(), ty: Type::Number } }
    fn edge(fc: u32, fp: u32, tc: u32, tp: u32) -> Edge {
        Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp }
    }
    /// A primitive with `n_in` input ports (typed `Any`, irrelevant at eval) and an output.
    fn prim(name: &str, n_in: usize, out: Type) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![],
            inputs: (0..n_in).map(|i| Port { name: format!("i{i}"), ty: Type::Any }).collect(),
            outputs: vec![Port { name: "out".into(), ty: out }],
        }
    }
    fn composite(inputs: Vec<Port>, out: Type, children: Vec<Hash>, edges: Vec<Edge>) -> Unit {
        Unit {
            body: UnitBody::Composite { children, edges },
            params: vec![],
            inputs,
            outputs: vec![Port { name: "out".into(), ty: out }],
        }
    }
    /// A composite exposing several outputs — `composite()`'s multi-output sibling, named
    /// `out0`, `out1`, … in declaration order.
    fn multi_composite(inputs: Vec<Port>, outs: Vec<Type>, children: Vec<Hash>, edges: Vec<Edge>) -> Unit {
        Unit {
            body: UnitBody::Composite { children, edges },
            params: vec![],
            inputs,
            outputs: outs
                .into_iter()
                .enumerate()
                .map(|(i, ty)| Port { name: format!("out{i}"), ty })
                .collect(),
        }
    }
    /// input a → add.0 ; input b → add.1 ; add.out → output. Returns the composite.
    fn add2(r: &mut MapResolver) -> Unit {
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        composite(
            vec![nport("a"), nport("b")],
            Type::Number,
            vec![hadd],
            vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, BOUNDARY, 0)],
        )
    }

    #[test]
    fn eval_primitive_add() {
        let r = MapResolver::new();
        let add = prim("add", 3, Type::Number);
        assert_eq!(
            eval(&add, &[Some(num(3.0)), Some(num(7.0)), Some(num(5.0))], &r).into_vec(),
            vec![Some(num(15.0))]
        );
    }

    /// Regression: absent inputs used to be dropped before `compute`, so a variadic leaf fired
    /// on whatever was left — `and(true, <absent>)` was `true`, an auth check failing open.
    #[test]
    fn coalesce_fires_on_an_absent_wired_input_and_takes_the_first_that_arrived() {
        let r = MapResolver::new();
        let leaf = prim("coalesce", 2, Type::Any);
        let (a, b) = (num(1.0), num(2.0));
        assert_eq!(eval(&leaf, &[Some(a.clone()), Some(b.clone())], &r).into_vec(), vec![Some(a.clone())]);
        assert_eq!(eval(&leaf, &[None, Some(b.clone())], &r).into_vec(), vec![Some(b.clone())]);
        assert_eq!(eval(&leaf, &[Some(a.clone()), None], &r).into_vec(), vec![Some(a.clone())]);
        assert_eq!(eval(&leaf, &[None, None], &r).into_vec(), vec![None]);

        // Inside a composite, the case the language lowers `x ?? y` to: `a` comes from a node
        // that produced nothing (head of an empty list), `b` from one that did. Both wired, one
        // absent: coalesce still fires, and answers b. The same wiring with any other leaf in
        // coalesce's place produces nothing (the firing rule), which is why it is a leaf.
        let mut r = MapResolver::new();
        let empty = r.insert(prim("list", 0, Type::list_any())).unwrap();
        let head = r.insert(prim("head", 1, Type::Any)).unwrap();
        let fallback = r.insert(Unit { params: vec![Param::frozen("value", "7"), Param::frozen("type", "Number")], ..prim("const", 0, Type::Number) }).unwrap();
        let pick = r.insert(leaf.clone()).unwrap();
        let wiring = vec![edge(0, 0, 1, 0), edge(1, 0, 3, 0), edge(2, 0, 3, 1), edge(3, 0, BOUNDARY, 0)];
        let c = composite(vec![], Type::Any, vec![empty, head, fallback, pick], wiring.clone());
        assert_eq!(eval(&c, &[], &r).into_vec(), vec![Some(num(7.0))]);
        let with_add = composite(vec![], Type::Any, vec![empty, head, fallback, r.insert(prim("add", 2, Type::Number)).unwrap()], wiring);
        assert_eq!(eval(&with_add, &[], &r).into_vec(), vec![None]);
    }

    #[test]
    fn variadic_leaf_with_an_absent_input_fails_closed() {
        let r = MapResolver::new();
        let t = Value::Boolean(true);
        let x = Value::Text("x".into());
        for (name, present) in [
            ("and", t.clone()),
            ("or", Value::Boolean(false)),
            ("add", num(1.0)),
            ("multiply", num(2.0)),
            ("concat", x.clone()),
            ("list", x.clone()),
        ] {
            let leaf = prim(name, 2, Type::Any);
            assert_eq!(eval(&leaf, &[Some(present.clone()), None], &r).into_vec(), vec![None], "{name}(v, <absent>)");
            assert_eq!(eval(&leaf, &[None, Some(present)], &r).into_vec(), vec![None], "{name}(<absent>, v)");
            assert_eq!(eval(&leaf, &[None, None], &r).into_vec(), vec![None], "{name}(<absent>, <absent>)");
        }
        // Inside a composite, an unwired declared port is absent too.
        let mut r = MapResolver::new();
        let hand = r.insert(prim("and", 2, Type::Boolean)).unwrap();
        let c = composite(vec![], Type::Boolean, vec![hand], vec![edge(0, 0, BOUNDARY, 0)]);
        assert_eq!(eval(&c, &[], &r).into_vec(), vec![None]);
        // All present still fires, and a zero-input variadic leaf has nothing absent.
        assert_eq!(eval(&prim("and", 2, Type::Boolean), &[Some(t.clone()), Some(t.clone())], &r).into_vec(), vec![Some(t)]);
        assert_eq!(eval(&prim("list", 0, Type::Any), &[], &r).into_vec(), vec![Some(Value::List(vec![]))]);
    }

    /// Regression: an edge whose `from_child` names no child indexed `scratch` out of bounds.
    /// `validate` rejects such a unit, but `eval` is reached without it (the wasm bridge's
    /// `Evaluator::eval`, `verify_entry` on a peer's cache entry), so it must not panic.
    #[test]
    fn edge_from_a_missing_child_reads_as_absent_not_a_panic() {
        let mut r = MapResolver::new();
        let hnot = r.insert(prim("not", 1, Type::Boolean)).unwrap();
        let c = composite(
            vec![],
            Type::Any,
            vec![hnot],
            vec![edge(5, 0, 0, 0), edge(0, 0, BOUNDARY, 0)], // child 5 does not exist
        );
        assert_eq!(eval(&c, &[], &r).into_vec(), vec![None]);
        // An exposed output fed straight from a missing child, and from a missing port.
        let c = composite(vec![], Type::Any, vec![hnot], vec![edge(9, 0, BOUNDARY, 0)]);
        assert_eq!(eval(&c, &[], &r).into_vec(), vec![None]);
        let c = composite(vec![], Type::Any, vec![hnot], vec![edge(0, 3, BOUNDARY, 0)]);
        assert_eq!(eval(&c, &[], &r).into_vec(), vec![None]);
    }

    #[test]
    fn an_observer_sees_each_composite_child_by_path_and_changes_nothing() {
        #[derive(Default)]
        struct Paths(Vec<String>);
        impl Observer for Paths {
            fn enter(&mut self, path: &[u32]) {
                self.0.push(format!("+{path:?}"));
            }
            fn leave(&mut self, path: &[u32]) {
                self.0.push(format!("-{path:?}"));
            }
            fn produced(&mut self, path: &[u32], outputs: &[Slot]) {
                self.0.push(format!("={path:?}{outputs:?}"));
            }
        }
        let mut r = MapResolver::new();
        let inner = add2(&mut r); // a composite holding one add
        let hinner = r.insert(inner).unwrap();
        let hneg = r.insert(prim("multiply", 2, Type::Number)).unwrap();
        let outer = composite(
            vec![nport("a"), nport("b")],
            Type::Number,
            vec![hinner, hneg],
            vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, 1, 0), edge(BOUNDARY, 1, 1, 1), edge(1, 0, BOUNDARY, 0)],
        );
        let inputs = [Some(num(3.0)), Some(num(4.0))];
        let mut seen = Paths::default();
        let observed = eval_observed(&outer, &inputs, &r, &mut seen);
        assert_eq!(observed, eval(&outer, &inputs, &r), "observing changes nothing");
        assert_eq!(observed.into_vec(), vec![Some(num(28.0))]);
        let n = |x: f64| format!("[Value(Number({x:?}))]");
        let expected = [
            "+[0]".to_string(),
            "+[0, 0]".into(),
            format!("=[0, 0]{}", n(7.0)),
            "-[0, 0]".into(),
            format!("=[0]{}", n(7.0)),
            "-[0]".into(),
            "+[1]".into(),
            format!("=[1]{}", n(28.0)),
            "-[1]".into(),
        ];
        assert_eq!(seen.0, expected, "each node's outputs, just before it is left");
    }

    /// An observer may stand a node's outputs in for what core would compute (`asserted`), and
    /// the rest of the run uses them; an answer with the wrong number of outputs is ignored.
    #[test]
    fn an_observer_may_assert_a_nodes_outputs() {
        struct Asserts(Vec<u32>, Vec<Option<Value>>, Vec<(Vec<u32>, Vec<Option<Value>>)>);
        impl Observer for Asserts {
            fn asserted(&mut self, path: &[u32], _unit: &Unit, inputs: &[Option<Value>]) -> Option<Vec<Option<Value>>> {
                self.2.push((path.to_vec(), inputs.to_vec()));
                (path == self.0.as_slice()).then(|| self.1.clone())
            }
        }
        let mut r = MapResolver::new();
        let inner = add2(&mut r);
        let hinner = r.insert(inner).unwrap();
        let hmul = r.insert(prim("multiply", 2, Type::Number)).unwrap();
        let outer = composite(
            vec![nport("a"), nport("b")],
            Type::Number,
            vec![hinner, hmul],
            vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, 1, 0), edge(BOUNDARY, 1, 1, 1), edge(1, 0, BOUNDARY, 0)],
        );
        let inputs = [Some(num(3.0)), Some(num(4.0))];
        // The add at [0] is said to have made 10 (it makes 7): the multiply after it uses 10.
        let mut o = Asserts(vec![0], vec![Some(num(10.0))], Vec::new());
        assert_eq!(eval_observed(&outer, &inputs, &r, &mut o).into_vec(), vec![Some(num(40.0))]);
        assert_eq!(o.2[0], (vec![0], vec![Some(num(3.0)), Some(num(4.0))]), "asked with the node's gathered inputs");
        assert!(!o.2.iter().any(|(p, _)| p == &[0, 0]), "a node stood in for isn't evaluated inside");
        // Two outputs for a one-output node: ignored, and the node computed.
        let mut o = Asserts(vec![0], vec![Some(num(10.0)), None], Vec::new());
        assert_eq!(eval_observed(&outer, &inputs, &r, &mut o).into_vec(), vec![Some(num(28.0))]);
        // Nothing stood in for: as plain eval.
        let mut o = Asserts(vec![9], vec![], Vec::new());
        assert_eq!(eval_observed(&outer, &inputs, &r, &mut o), eval(&outer, &inputs, &r));
    }

    /// `path → http_request → field(status)`: the observer is asked for the request's answer by
    /// the node's path; its answer flows downstream. Plain `eval` has no one to answer, so the
    /// response, and everything after it, is absent.
    #[test]
    fn an_http_request_is_answered_by_the_observer_or_stays_absent() {
        use std::collections::BTreeMap;
        struct Answers(Vec<(Vec<u32>, Value)>);
        impl Observer for Answers {
            fn answer(&mut self, path: &[u32], request: &Value) -> Answer {
                self.0.push((path.to_vec(), request.clone()));
                Answer::Value(Value::Record(BTreeMap::from([("status".to_string(), num(201.0))])))
            }
        }
        let mut r = MapResolver::new();
        let request = Unit {
            params: vec![Param::frozen("method", "POST"), Param::frozen("host", "api.example.com")],
            ..prim("http_request", 3, Type::Record)
        };
        let field = Unit { params: vec![Param::frozen("name", "status")], ..prim("field", 1, Type::Any) };
        let (hreq, hfield) = (r.insert(request).unwrap(), r.insert(field).unwrap());
        let flow = composite(
            vec![Port { name: "path".into(), ty: Type::Text }],
            Type::Any,
            vec![hreq, hfield],
            vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, 1, 0), edge(1, 0, BOUNDARY, 0)],
        );
        let inputs = [Some(Value::Text("/items".into()))];
        let mut answers = Answers(Vec::new());
        assert_eq!(eval_observed(&flow, &inputs, &r, &mut answers).into_vec(), vec![Some(num(201.0))]);
        assert_eq!(answers.0.len(), 1);
        let (path, req) = &answers.0[0];
        assert_eq!(path, &vec![0]);
        let Value::Record(req) = req else { panic!("{req:?}") };
        assert_eq!(req["url"], Value::Text("https://api.example.com/items".into()));
        assert_eq!(eval(&flow, &inputs, &r).into_vec(), vec![None], "no one to answer: absent");
    }

    /// Regression: an `http_request` whose wired body was absent (a webhook that failed its
    /// signature check upstream) still went out, with an empty body. Unwired, the body is
    /// optional ("none"); wired, its absence holds the request back, like any input.
    #[test]
    fn a_wired_but_absent_body_holds_an_http_request_back() {
        struct Count(usize);
        impl Observer for Count {
            fn answer(&mut self, _: &[u32], _: &Value) -> Answer {
                self.0 += 1;
                Answer::Value(num(200.0))
            }
        }
        let mut r = MapResolver::new();
        let request = Unit {
            params: vec![Param::frozen("method", "POST"), Param::frozen("host", "hooks.slack.com")],
            ..prim("http_request", 3, Type::Record)
        };
        let hreq = r.insert(request).unwrap();
        let ports = vec![Port { name: "path".into(), ty: Type::Text }, Port { name: "body".into(), ty: Type::Any }];
        let wired = composite(ports.clone(), Type::Any, vec![hreq], vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 2), edge(0, 0, BOUNDARY, 0)]);
        let unwired = composite(ports, Type::Any, vec![hreq], vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, BOUNDARY, 0)]);
        let path = Some(Value::Text("/services/x".into()));
        let send = |flow: &Unit, body: Option<Value>| {
            let mut sent = Count(0);
            let out = eval_observed(flow, &[path.clone(), body], &r, &mut sent).into_vec();
            (out, sent.0)
        };
        assert_eq!(send(&wired, Some(Value::Text("hi".into()))), (vec![Some(num(200.0))], 1));
        assert_eq!(send(&wired, None), (vec![None], 0), "wired, absent: not sent");
        assert_eq!(send(&unwired, None), (vec![Some(num(200.0))], 1), "unwired: optional, sent without a body");
    }

    #[test]
    fn eval_composite_add() {
        let mut r = MapResolver::new();
        let c = add2(&mut r);
        assert_eq!(eval(&c, &[Some(num(3.0)), Some(num(4.0))], &r).into_vec(), vec![Some(num(7.0))]);
    }

    #[test]
    fn promoted_input_routes_straight_to_output() {
        // A composite whose output *is* its input (an identity passthrough): no
        // children, a single BOUNDARY→BOUNDARY edge. Valid per validate; eval must
        // return the input rather than index the scratch by BOUNDARY and panic.
        let r = MapResolver::new();
        let c = composite(vec![nport("x")], Type::Number, vec![], vec![edge(BOUNDARY, 0, BOUNDARY, 0)]);
        assert_eq!(eval(&c, &[Some(num(42.0))], &r).into_vec(), vec![Some(num(42.0))]);
    }

    #[test]
    fn eval_composite_sum() {
        // The v3 seed (minus Map): list(a,b,c) → sum → 15.
        let mut r = MapResolver::new();
        let hlist = r.insert(prim("list", 3, Type::list_any())).unwrap();
        let hsum = r.insert(prim("sum", 1, Type::Number)).unwrap();
        let c = composite(
            vec![nport("a"), nport("b"), nport("c")],
            Type::Number,
            vec![hlist, hsum],
            vec![
                edge(BOUNDARY, 0, 0, 0),
                edge(BOUNDARY, 1, 0, 1),
                edge(BOUNDARY, 2, 0, 2),
                edge(0, 0, 1, 0),       // list → sum
                edge(1, 0, BOUNDARY, 0), // sum → output
            ],
        );
        assert_eq!(
            eval(&c, &[Some(num(3.0)), Some(num(7.0)), Some(num(5.0))], &r).into_vec(),
            vec![Some(num(15.0))]
        );
    }

    #[test]
    fn eval_nested_composite() {
        let mut r = MapResolver::new();
        let inner = add2(&mut r);
        let hinner = r.insert(inner).unwrap();
        let outer = composite(
            vec![nport("x"), nport("y")],
            Type::Number,
            vec![hinner],
            vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, BOUNDARY, 0)],
        );
        assert_eq!(eval(&outer, &[Some(num(10.0)), Some(num(5.0))], &r).into_vec(), vec![Some(num(15.0))]);
    }

    #[test]
    fn none_propagates() {
        // subtract is binary; one operand absent ⇒ compute None ⇒ output None.
        let mut r = MapResolver::new();
        let hsub = r.insert(prim("subtract", 2, Type::Number)).unwrap();
        let c = composite(
            vec![nport("a"), nport("b")],
            Type::Number,
            vec![hsub],
            vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, BOUNDARY, 0)],
        );
        assert_eq!(eval(&c, &[Some(num(3.0)), None], &r).into_vec(), vec![None]);
    }

    // hex-encode a hash for a `map` fn param (the inverse of hash::parse_hash).
    fn hex(h: &Hash) -> String {
        h.iter().map(|b| format!("{:02x}", b)).collect()
    }
    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn list(xs: Vec<Value>) -> Value {
        Value::List(xs)
    }
    // Double(x) = x * 2, as a composite: BOUNDARY.x → mul.0 ; const(2) → mul.1 ; mul → out.
    fn double(r: &mut MapResolver) -> Hash {
        let two = Unit {
            body: UnitBody::Primitive { name: "const".into() },
            params: vec![Param::frozen("type", "Number"), Param::frozen("value", "2")],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        };
        let htwo = r.insert(two).unwrap();
        let hmul = r.insert(prim("multiply", 2, Type::Number)).unwrap();
        let unit = composite(
            vec![nport("x")],
            Type::Number,
            vec![hmul, htwo], // child0 = mul, child1 = const 2
            vec![edge(BOUNDARY, 0, 0, 0), edge(1, 0, 0, 1), edge(0, 0, BOUNDARY, 0)],
        );
        r.insert(unit).unwrap()
    }
    fn map_unit(fn_hash: &Hash) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![Param::frozen("fn", hex(fn_hash))],
            inputs: vec![Port { name: "list".into(), ty: Type::list_any() }],
            outputs: vec![Port { name: "out".into(), ty: Type::list_any() }],
        }
    }

    /// times(x, k) = x * k: a function of the item and one value from around it.
    fn times(r: &mut MapResolver) -> Hash {
        let hmul = r.insert(prim("multiply", 2, Type::Number)).unwrap();
        let unit = composite(vec![nport("x"), nport("k")], Type::Number, vec![hmul], vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, BOUNDARY, 0)]);
        r.insert(unit).unwrap()
    }

    /// Inputs after the list reach every application after the item, the same each time; one
    /// absent, nothing fires.
    #[test]
    fn map_and_fold_pass_extra_inputs_to_their_function() {
        let mut r = MapResolver::new();
        let htimes = times(&mut r);
        let mut m = map_unit(&htimes);
        m.inputs.push(Port { name: "in2".into(), ty: Type::Any });
        let xs = list(vec![num(1.0), num(2.0), num(3.0)]);
        assert_eq!(eval(&m, &[Some(xs.clone()), Some(num(10.0))], &r).into_vec(), vec![Some(list(vec![num(10.0), num(20.0), num(30.0)]))]);
        assert_eq!(eval(&m, &[Some(xs.clone()), None], &r).into_vec(), vec![None]);
        // fold(acc, item, k) = acc + item * k, as a composite: mul(item, k) → add(acc, ·).
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let hmul = r.insert(prim("multiply", 2, Type::Number)).unwrap();
        let step = composite(
            vec![nport("acc"), nport("item"), nport("k")],
            Type::Number,
            vec![hmul, hadd],
            vec![edge(BOUNDARY, 1, 0, 0), edge(BOUNDARY, 2, 0, 1), edge(BOUNDARY, 0, 1, 0), edge(0, 0, 1, 1), edge(1, 0, BOUNDARY, 0)],
        );
        let hstep = r.insert(step).unwrap();
        let mut f = fold_unit(&hstep);
        f.inputs.push(Port { name: "in3".into(), ty: Type::Any });
        assert_eq!(eval(&f, &[Some(num(0.0)), Some(xs.clone()), Some(num(2.0))], &r).into_vec(), vec![Some(num(12.0))]);
        assert_eq!(eval(&f, &[Some(num(0.0)), Some(xs), None], &r).into_vec(), vec![None]);
    }

    /// A read inside a mapped function is asked of the host, at the map's path then the item's
    /// index, each item's on its own; the application's steps aren't reported. A fold's alike.
    #[test]
    fn a_read_inside_map_or_fold_is_answered_at_the_items_path() {
        use std::collections::BTreeMap;
        #[derive(Default)]
        struct Prices {
            asked: Vec<(Vec<u32>, Value)>,
            entered: Vec<Vec<u32>>,
        }
        impl Observer for Prices {
            fn enter(&mut self, path: &[u32]) {
                self.entered.push(path.to_vec());
            }
            fn answer(&mut self, path: &[u32], request: &Value) -> Answer {
                self.asked.push((path.to_vec(), request.clone()));
                let Value::Record(r) = request else { return Answer::Absent };
                match &r["get"] {
                    Value::Text(k) if k == "tea" => Answer::Value(num(4.0)),
                    Value::Text(k) if k == "cake" => Answer::Value(num(3.0)),
                    _ => Answer::Absent,
                }
            }
        }
        let mut r = MapResolver::new();
        let get = Unit { params: vec![Param::frozen("collection", "prices")], ..prim("collection_get", 1, Type::Any) };
        let hget = r.insert(get).unwrap();
        // price(key) = prices[key], one child deep.
        let price = composite(vec![Port { name: "key".into(), ty: Type::Text }], Type::Any, vec![hget], vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, BOUNDARY, 0)]);
        let hprice = r.insert(price).unwrap();
        let hmap = r.insert(map_unit(&hprice)).unwrap();
        // The flow: its one child is the map, at [0].
        let flow = composite(vec![Port { name: "keys".into(), ty: Type::list_any() }], Type::list_any(), vec![hmap], vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, BOUNDARY, 0)]);
        let keys = list(vec![Value::Text("tea".into()), Value::Text("cake".into())]);
        let mut seen = Prices::default();
        assert_eq!(eval_observed(&flow, &[Some(keys.clone())], &r, &mut seen).into_vec(), vec![Some(list(vec![num(4.0), num(3.0)]))]);
        let get = |k: &str| Value::Record(BTreeMap::from([("collection".to_string(), Value::Text("prices".into())), ("get".to_string(), Value::Text(k.into()))]));
        assert_eq!(seen.asked, vec![(vec![0, 0, 0], get("tea")), (vec![0, 1, 0], get("cake"))], "map [0], item k, the read at child 0");
        assert_eq!(seen.entered, vec![vec![0]], "only the map itself is a step");
        assert_eq!(eval(&flow, &[Some(keys.clone())], &r).into_vec(), vec![None], "no one to answer: absent, as outside a map");
        // fold(0, keys, fn: (acc, key) -> acc + prices[key]).
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let step = composite(
            vec![nport("acc"), Port { name: "key".into(), ty: Type::Text }],
            Type::Number,
            vec![hget, hadd],
            vec![edge(BOUNDARY, 1, 0, 0), edge(BOUNDARY, 0, 1, 0), edge(0, 0, 1, 1), edge(1, 0, BOUNDARY, 0)],
        );
        let hstep = r.insert(step).unwrap();
        let mut seen = Prices::default();
        assert_eq!(eval_observed(&fold_unit(&hstep), &[Some(num(0.0)), Some(keys)], &r, &mut seen).into_vec(), vec![Some(num(7.0))]);
        assert_eq!(seen.asked.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(), vec![vec![0, 0], vec![1, 0]], "item k, the read at child 0");
    }

    #[test]
    fn map_doubles_list() {
        // The v3 seed: list(3,7,5) → Map(Double) → [6, 14, 10].
        let mut r = MapResolver::new();
        let hdouble = double(&mut r);
        let m = map_unit(&hdouble);
        let input = list(vec![num(3.0), num(7.0), num(5.0)]);
        assert_eq!(
            eval(&m, &[Some(input)], &r).into_vec(),
            vec![Some(list(vec![num(6.0), num(14.0), num(10.0)]))]
        );
    }

    #[test]
    fn map_none_propagates() {
        // Double applied to a Text element fails ⇒ the whole map is None.
        let mut r = MapResolver::new();
        let hdouble = double(&mut r);
        let m = map_unit(&hdouble);
        let input = list(vec![num(3.0), txt("x")]);
        assert_eq!(eval(&m, &[Some(input)], &r).into_vec(), vec![None]);
    }

    fn fold_unit(fn_hash: &Hash) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "fold".into() },
            params: vec![Param::frozen("fn", hex(fn_hash))],
            inputs: vec![
                Port { name: "init".into(), ty: Type::Any },
                Port { name: "list".into(), ty: Type::list_any() },
            ],
            outputs: vec![Port { name: "out".into(), ty: Type::Any }],
        }
    }

    #[test]
    fn fold_sums_with_add() {
        // sum expressed as a fold (the universal donor): fold(add, 0, [3,7,5]) = 15.
        // `add` is the combiner — add(acc, elem) = acc + elem.
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let f = fold_unit(&hadd);
        let xs = list(vec![num(3.0), num(7.0), num(5.0)]);
        assert_eq!(eval(&f, &[Some(num(0.0)), Some(xs)], &r).into_vec(), vec![Some(num(15.0))]);
    }

    #[test]
    fn fold_on_empty_is_init() {
        // No element ⇒ the accumulator is the seed, untouched. Total.
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let f = fold_unit(&hadd);
        assert_eq!(eval(&f, &[Some(num(42.0)), Some(list(vec![]))], &r).into_vec(), vec![Some(num(42.0))]);
    }

    #[test]
    fn fold_none_propagates() {
        // add(acc, "x") fails ⇒ the whole fold is None.
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let f = fold_unit(&hadd);
        let xs = list(vec![num(3.0), txt("x")]);
        assert_eq!(eval(&f, &[Some(num(0.0)), Some(xs)], &r).into_vec(), vec![None]);
    }

    #[test]
    fn parity_standalone_primitive_eq_composite() {
        // A primitive and an equivalent composite evaluate identically in isolation.
        let mut r = MapResolver::new();
        let comp_add = add2(&mut r);
        let prim_add = prim("add", 2, Type::Number);
        let inp = [Some(num(8.0)), Some(num(9.0))];
        assert_eq!(eval(&prim_add, &inp, &r), eval(&comp_add, &inp, &r));
        assert_eq!(eval(&comp_add, &inp, &r).into_vec(), vec![Some(num(17.0))]);
    }

    #[test]
    fn substitution_in_host_is_invisible() {
        // P1.2.1 / I1: swap a host's child between a *primitive* add and a
        // *composite-wrapping-add* (genuinely different units — different hashes) and the
        // host's output is identical. This is the substitutability claim, not just
        // standalone equality. Host computes (x + y) * z via the swappable add-like child.
        let mut r = MapResolver::new();
        let h_prim = r.insert(prim("add", 2, Type::Number)).unwrap();
        let comp_add = add2(&mut r);
        let h_comp = r.insert(comp_add).unwrap();
        let h_mul = r.insert(prim("multiply", 2, Type::Number)).unwrap();

        // child0 is the swappable "add-like" unit; child1 is multiply.
        let host = |child0: Hash| {
            composite(
                vec![nport("x"), nport("y"), nport("z")],
                Type::Number,
                vec![child0, h_mul],
                vec![
                    edge(BOUNDARY, 0, 0, 0), // x → add-like.0
                    edge(BOUNDARY, 1, 0, 1), // y → add-like.1
                    edge(0, 0, 1, 0),        // add-like.out → mul.0
                    edge(BOUNDARY, 2, 1, 1), // z → mul.1
                    edge(1, 0, BOUNDARY, 0), // mul.out → output
                ],
            )
        };

        assert_ne!(h_prim, h_comp, "the two children must be genuinely different units");

        let host_prim = host(h_prim);
        let host_comp = host(h_comp);
        let inp = [Some(num(3.0)), Some(num(4.0)), Some(num(2.0))]; // (3 + 4) * 2 = 14
        assert_eq!(eval(&host_prim, &inp, &r), eval(&host_comp, &inp, &r));
        assert_eq!(eval(&host_prim, &inp, &r).into_vec(), vec![Some(num(14.0))]);
    }

    #[test]
    fn deterministic() {
        let mut r = MapResolver::new();
        let c = add2(&mut r);
        let inp = [Some(num(3.0)), Some(num(4.0))];
        assert_eq!(eval(&c, &inp, &r), eval(&c, &inp, &r));
    }

    // ── P4.7: native multi-output (composite-only) ──────────────────────────

    #[test]
    fn eval_composite_multi_output() {
        // Three independent ops fed the same two inputs, all three exposed — proving a
        // multi-output composite genuinely carries several distinct values, not one value
        // dressed up in a longer vec.
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let hmul = r.insert(prim("multiply", 2, Type::Number)).unwrap();
        let hsub = r.insert(prim("subtract", 2, Type::Number)).unwrap();
        let c = multi_composite(
            vec![nport("a"), nport("b")],
            vec![Type::Number, Type::Number, Type::Number],
            vec![hadd, hmul, hsub],
            vec![
                edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), // a, b → add
                edge(BOUNDARY, 0, 1, 0), edge(BOUNDARY, 1, 1, 1), // a, b → multiply
                edge(BOUNDARY, 0, 2, 0), edge(BOUNDARY, 1, 2, 1), // a, b → subtract
                edge(0, 0, BOUNDARY, 0), // add    → out0
                edge(1, 0, BOUNDARY, 1), // multiply → out1
                edge(2, 0, BOUNDARY, 2), // subtract → out2
            ],
        );
        assert_eq!(
            eval(&c, &[Some(num(3.0)), Some(num(4.0))], &r).into_vec(),
            vec![Some(num(7.0)), Some(num(12.0)), Some(num(-1.0))]
        );
    }

    #[test]
    fn multi_output_partial_success() {
        // Two exposed outputs from two different children — one succeeds, one fails (divide by
        // zero) — independently. This is the actual reason eval's return shape is per-slot
        // (EvalOutputs, one Option per declared output) rather than Option<Vec<Value>>: a
        // genuinely computed value is never discarded just because a sibling output failed.
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let hdiv = r.insert(prim("divide", 2, Type::Number)).unwrap();
        let c = multi_composite(
            vec![nport("a"), nport("b")],
            vec![Type::Number, Type::Number],
            vec![hadd, hdiv],
            vec![
                edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), // a, b → add
                edge(BOUNDARY, 0, 1, 0), edge(BOUNDARY, 1, 1, 1), // a, b → divide
                edge(0, 0, BOUNDARY, 0), // add    → out0
                edge(1, 0, BOUNDARY, 1), // divide → out1
            ],
        );
        assert_eq!(
            eval(&c, &[Some(num(3.0)), Some(num(0.0))], &r).into_vec(),
            vec![Some(num(3.0)), None] // add(3,0)=3 succeeds; divide(3,0) fails — independently
        );
    }

    #[test]
    fn eval_one_requires_exactly_one_output() {
        // The convenience map/fold's fn uses: fails closed for a multi-output unit (never
        // silently takes the first of several and discards the rest), and matches eval(...)[0]
        // exactly for a genuine single-output unit.
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let hmul = r.insert(prim("multiply", 2, Type::Number)).unwrap();
        let multi = multi_composite(
            vec![nport("a"), nport("b")],
            vec![Type::Number, Type::Number],
            vec![hadd, hmul],
            vec![
                edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1),
                edge(BOUNDARY, 0, 1, 0), edge(BOUNDARY, 1, 1, 1),
                edge(0, 0, BOUNDARY, 0),
                edge(1, 0, BOUNDARY, 1),
            ],
        );
        assert_eq!(eval_one(&multi, &[Some(num(3.0)), Some(num(4.0))], &r), None);

        let single = prim("add", 2, Type::Number);
        assert_eq!(eval_one(&single, &[Some(num(3.0)), Some(num(4.0))], &r), Some(num(7.0)));
    }

    // ── Lazy choices (docs/L4s/lazy-choices-L4.md) ──

    /// Answers by path from a table, and `NotYet` for any path it has no answer for, as a round's
    /// host does for a request it hasn't performed. Keeps what it was asked and what it entered.
    #[derive(Default)]
    struct Round {
        answers: BTreeMap<Vec<u32>, Answer>,
        asked: Vec<Vec<u32>>,
        entered: Vec<Vec<u32>>,
    }
    impl Round {
        fn with(answers: &[(&[u32], Answer)]) -> Round {
            Round { answers: answers.iter().map(|(p, a)| (p.to_vec(), a.clone())).collect(), ..Round::default() }
        }
    }
    impl Observer for Round {
        fn enter(&mut self, path: &[u32]) {
            self.entered.push(path.to_vec());
        }
        fn answer(&mut self, path: &[u32], _: &Value) -> Answer {
            self.asked.push(path.to_vec());
            self.answers.get(path).cloned().unwrap_or(Answer::NotYet)
        }
    }

    /// An `http_request` GET of its one input, a path, on api.example.com.
    fn get(r: &mut MapResolver) -> Hash {
        let request = Unit { params: vec![Param::frozen("method", "GET"), Param::frozen("host", "api.example.com")], ..prim("http_request", 3, Type::Record) };
        r.insert(request).unwrap()
    }
    fn tport(name: &str) -> Port {
        Port { name: name.into(), ty: Type::Text }
    }
    fn bport(name: &str) -> Port {
        Port { name: name.into(), ty: Type::Boolean }
    }
    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    /// `if go then http.get(path)`: `guard(go, request)`. The request is asked only when `go`.
    #[test]
    fn a_request_under_a_false_guard_is_never_asked() {
        let mut r = MapResolver::new();
        let (hget, hguard) = (get(&mut r), r.insert(prim("guard", 2, Type::Any)).unwrap());
        let flow = composite(vec![bport("go"), tport("path")], Type::Any, vec![hget, hguard], vec![edge(BOUNDARY, 1, 0, 0), edge(BOUNDARY, 0, 1, 0), edge(0, 0, 1, 1), edge(1, 0, BOUNDARY, 0)]);
        let answered = Round::with(&[(&[0], Answer::Value(num(1.0)))]);

        let mut round = Round { answers: answered.answers.clone(), ..Round::default() };
        let out = eval_observed(&flow, &[Some(Value::Boolean(false)), Some(text("/x"))], &r, &mut round);
        assert_eq!(out.into_vec(), vec![None]);
        assert!(round.asked.is_empty(), "the branch not taken asks nothing");
        assert_eq!(round.entered, vec![vec![1]], "only the guard runs");

        let mut round = answered;
        let out = eval_observed(&flow, &[Some(Value::Boolean(true)), Some(text("/x"))], &r, &mut round);
        assert_eq!(out.into_vec(), vec![Some(num(1.0))]);
        assert_eq!(round.asked, vec![vec![0]]);
    }

    /// `a ?? http.get(path)`: the request is asked only when `a` is absent.
    #[test]
    fn a_request_under_coalesce_is_asked_only_when_the_first_is_absent() {
        let mut r = MapResolver::new();
        let (hget, hco) = (get(&mut r), r.insert(prim("coalesce", 2, Type::Any)).unwrap());
        let flow = composite(vec![nport("a"), tport("path")], Type::Any, vec![hget, hco], vec![edge(BOUNDARY, 1, 0, 0), edge(BOUNDARY, 0, 1, 0), edge(0, 0, 1, 1), edge(1, 0, BOUNDARY, 0)]);

        let mut round = Round::with(&[(&[0], Answer::Value(num(2.0)))]);
        assert_eq!(eval_observed(&flow, &[Some(num(1.0)), Some(text("/x"))], &r, &mut round).into_vec(), vec![Some(num(1.0))]);
        assert!(round.asked.is_empty());

        let mut round = Round::with(&[(&[0], Answer::Value(num(2.0)))]);
        assert_eq!(eval_observed(&flow, &[None, Some(text("/x"))], &r, &mut round).into_vec(), vec![Some(num(2.0))]);
        assert_eq!(round.asked, vec![vec![0]]);
    }

    /// `select` is strict (an absent side makes it absent), so it needs, and asks, both sides.
    #[test]
    fn select_is_not_a_choice_and_asks_both_sides() {
        let mut r = MapResolver::new();
        let (hget, hsel) = (get(&mut r), r.insert(prim("select", 3, Type::Any)).unwrap());
        let flow = composite(vec![bport("c"), tport("path")], Type::Any, vec![hget, hsel], vec![edge(BOUNDARY, 1, 0, 0), edge(BOUNDARY, 0, 1, 0), edge(BOUNDARY, 1, 1, 1), edge(0, 0, 1, 2), edge(1, 0, BOUNDARY, 0)]);
        let mut round = Round::with(&[(&[0], Answer::Value(num(2.0)))]);
        assert_eq!(eval_observed(&flow, &[Some(Value::Boolean(true)), Some(text("/x"))], &r, &mut round).into_vec(), vec![Some(text("/x"))]);
        assert_eq!(round.asked, vec![vec![0]]);
        assert_eq!(choice_next("select", &[None, None, None]), None);
    }

    /// `guard(go, h(x, y))` with `h` a holon: not entered when `go` is false, nor anything in it.
    #[test]
    fn a_holon_under_an_untaken_guard_is_never_entered() {
        let mut r = MapResolver::new();
        let inner = add2(&mut r);
        let (hinner, hguard) = (r.insert(inner).unwrap(), r.insert(prim("guard", 2, Type::Any)).unwrap());
        let flow = composite(
            vec![bport("go"), nport("x"), nport("y")],
            Type::Any,
            vec![hinner, hguard],
            vec![edge(BOUNDARY, 1, 0, 0), edge(BOUNDARY, 2, 0, 1), edge(BOUNDARY, 0, 1, 0), edge(0, 0, 1, 1), edge(1, 0, BOUNDARY, 0)],
        );
        let mut round = Round::default();
        assert_eq!(eval_observed(&flow, &[Some(Value::Boolean(false)), Some(num(1.0)), Some(num(2.0))], &r, &mut round).into_vec(), vec![None]);
        assert_eq!(round.entered, vec![vec![1]]);
        let mut round = Round::default();
        assert_eq!(eval_observed(&flow, &[Some(Value::Boolean(true)), Some(num(1.0)), Some(num(2.0))], &r, &mut round).into_vec(), vec![Some(num(3.0))]);
        assert_eq!(round.entered, vec![vec![0], vec![0, 0], vec![1]], "what the guard needed, then the guard");
    }

    /// A node placed on its own (nothing consumes it, as on a canvas) still runs.
    #[test]
    fn a_node_nobody_consumes_is_still_asked() {
        let mut r = MapResolver::new();
        let hget = get(&mut r);
        let flow = composite(vec![tport("path")], Type::Text, vec![hget], vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 0, BOUNDARY, 0)]);
        let mut round = Round::default();
        assert_eq!(eval_observed(&flow, &[Some(text("/x"))], &r, &mut round).into_vec(), vec![Some(text("/x"))]);
        assert_eq!(round.asked, vec![vec![0]]);
    }

    /// `http.get(path) ?? http.get(path)`: the fallback waits until the first is known, and is
    /// asked only if the first comes back absent: a round later, not alongside it.
    #[test]
    fn coalesce_waits_on_a_pending_first_and_asks_its_fallback_only_once_it_is_absent() {
        let mut r = MapResolver::new();
        let (hget, hco) = (get(&mut r), r.insert(prim("coalesce", 2, Type::Any)).unwrap());
        let flow = composite(vec![tport("path")], Type::Any, vec![hget, hget, hco], vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 0, 1, 0), edge(0, 0, 2, 0), edge(1, 0, 2, 1), edge(2, 0, BOUNDARY, 0)]);
        let inputs = [Some(text("/x"))];

        // Round 1: nothing answered. Only the first is asked; the coalesce waits.
        let mut round = Round::default();
        assert_eq!(eval_observed(&flow, &inputs, &r, &mut round).into_vec(), vec![None]);
        assert_eq!(round.asked, vec![vec![0]]);
        assert_eq!(round.entered, vec![vec![0], vec![2]]);

        // Round 2, the first present: the fallback is never asked.
        let mut round = Round::with(&[(&[0], Answer::Value(num(1.0)))]);
        assert_eq!(eval_observed(&flow, &inputs, &r, &mut round).into_vec(), vec![Some(num(1.0))]);
        assert_eq!(round.asked, vec![vec![0]]);

        // Round 2, the first absent: now the fallback is asked, and answered in round 3.
        let mut round = Round::with(&[(&[0], Answer::Absent)]);
        assert_eq!(eval_observed(&flow, &inputs, &r, &mut round).into_vec(), vec![None]);
        assert_eq!(round.asked, vec![vec![0], vec![1]]);
        let mut round = Round::with(&[(&[0], Answer::Absent), (&[1], Answer::Value(num(2.0)))]);
        assert_eq!(eval_observed(&flow, &inputs, &r, &mut round).into_vec(), vec![Some(num(2.0))]);
    }

    /// `guard(http.get(path).ok, http.get(path))`: the value waits on the pending condition.
    #[test]
    fn a_guard_waits_on_a_pending_condition() {
        let mut r = MapResolver::new();
        let (hget, hguard) = (get(&mut r), r.insert(prim("guard", 2, Type::Any)).unwrap());
        let flow = composite(vec![tport("path")], Type::Any, vec![hget, hget, hguard], vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 0, 1, 0), edge(0, 0, 2, 0), edge(1, 0, 2, 1), edge(2, 0, BOUNDARY, 0)]);
        let inputs = [Some(text("/x"))];
        let mut round = Round::default();
        assert_eq!(eval_observed(&flow, &inputs, &r, &mut round).into_vec(), vec![None]);
        assert_eq!(round.asked, vec![vec![0]]);
        let mut round = Round::with(&[(&[0], Answer::Value(Value::Boolean(false)))]);
        assert_eq!(eval_observed(&flow, &inputs, &r, &mut round).into_vec(), vec![None]);
        assert_eq!(round.asked, vec![vec![0]], "false: the value is never asked");
        let mut round = Round::with(&[(&[0], Answer::Value(Value::Boolean(true))), (&[1], Answer::Value(num(5.0)))]);
        assert_eq!(eval_observed(&flow, &inputs, &r, &mut round).into_vec(), vec![Some(num(5.0))]);
        assert_eq!(round.asked, vec![vec![0], vec![1]]);
    }

    /// A holon, and a `map`, waiting on an answer are pending, not absent: a `coalesce` after them
    /// waits instead of asking its fallback.
    #[test]
    fn pending_crosses_a_holon_boundary_and_a_map() {
        let mut r = MapResolver::new();
        let (hget, hco) = (get(&mut r), r.insert(prim("coalesce", 2, Type::Any)).unwrap());
        // fetch(path) = http.get(path), one child deep.
        let fetch = composite(vec![tport("path")], Type::Any, vec![hget], vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, BOUNDARY, 0)]);
        let hfetch = r.insert(fetch).unwrap();
        let hmap = r.insert(map_unit(&hfetch)).unwrap();

        // fetch(path) ?? http.get(path)
        let flow = composite(vec![tport("path")], Type::Any, vec![hfetch, hget, hco], vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 0, 1, 0), edge(0, 0, 2, 0), edge(1, 0, 2, 1), edge(2, 0, BOUNDARY, 0)]);
        let mut round = Round::default();
        assert_eq!(eval_observed(&flow, &[Some(text("/x"))], &r, &mut round).into_vec(), vec![None]);
        assert_eq!(round.asked, vec![vec![0, 0]], "the holon's request, not the fallback");

        // map(paths, fetch) ?? http.get(path)
        let flow = composite(
            vec![Port { name: "paths".into(), ty: Type::list_any() }, tport("path")],
            Type::Any,
            vec![hmap, hget, hco],
            vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 1, 0), edge(0, 0, 2, 0), edge(1, 0, 2, 1), edge(2, 0, BOUNDARY, 0)],
        );
        let paths = Value::List(vec![text("/a"), text("/b")]);
        let mut round = Round::default();
        assert_eq!(eval_observed(&flow, &[Some(paths.clone()), Some(text("/x"))], &r, &mut round).into_vec(), vec![None]);
        assert_eq!(round.asked, vec![vec![0, 0, 0], vec![0, 1, 0]], "every item asked in the one round, not the fallback");
        // One item answered, one absent: the map is absent, and the fallback is asked.
        let mut round = Round::with(&[(&[0, 0, 0], Answer::Value(num(1.0))), (&[0, 1, 0], Answer::Absent), (&[1], Answer::Value(num(9.0)))]);
        assert_eq!(eval_observed(&flow, &[Some(paths), Some(text("/x"))], &r, &mut round).into_vec(), vec![Some(num(9.0))]);
    }

    #[test]
    fn the_choice_rule_needs_what_it_needs_and_decides_as_the_leaf_does() {
        let (t, f) = (Some(Slot::Value(Value::Boolean(true))), Some(Slot::Value(Value::Boolean(false))));
        let v = Some(Slot::Value(num(1.0)));
        assert_eq!(choice_next("guard", &[None, None]), Some(Choice::Need(0)));
        assert_eq!(choice_next("guard", &[t.clone(), None]), Some(Choice::Need(1)));
        assert_eq!(choice_next("guard", &[t, v.clone()]), Some(Choice::Done(Slot::Value(num(1.0)))));
        assert_eq!(choice_next("guard", &[f, None]), Some(Choice::Done(Slot::Absent)));
        assert_eq!(choice_next("guard", &[v.clone(), None]), Some(Choice::Done(Slot::Absent)), "a condition not a boolean");
        assert_eq!(choice_next("guard", &[Some(Slot::Pending), None]), Some(Choice::Done(Slot::Pending)));
        assert_eq!(choice_next("coalesce", &[None, None]), Some(Choice::Need(0)));
        assert_eq!(choice_next("coalesce", &[v.clone(), None]), Some(Choice::Done(Slot::Value(num(1.0)))));
        assert_eq!(choice_next("coalesce", &[Some(Slot::Absent), None]), Some(Choice::Need(1)));
        assert_eq!(choice_next("coalesce", &[Some(Slot::Absent), Some(Slot::Pending)]), Some(Choice::Done(Slot::Pending)));
        assert_eq!(choice_next("coalesce", &[Some(Slot::Pending), None]), Some(Choice::Done(Slot::Pending)));
        assert_eq!(choice_next("add", &[None, None]), None);
    }

    /// The property lazy choices rest on (L4 §2): for pure leaves, the lazy evaluator gives
    /// exactly what evaluating every node gives. Checked on generated graphs: random DAGs of
    /// choices and ordinary leaves, over inputs some of which are absent, some nested one level.
    #[test]
    fn lazy_and_eager_agree_on_generated_graphs() {
        use crate::eager::eval_eager;
        // A small deterministic generator (xorshift), so a failure reproduces from its seed.
        struct Rng(u64);
        impl Rng {
            fn next(&mut self) -> u64 {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                self.0
            }
            fn below(&mut self, n: usize) -> usize {
                (self.next() % n as u64) as usize
            }
        }
        const LEAVES: &[(&str, usize)] = &[("guard", 2), ("coalesce", 2), ("select", 3), ("add", 2), ("not", 1), ("equals", 2), ("and", 2), ("or", 2), ("is_truthy", 1)];
        let values = [None, Some(Value::Boolean(true)), Some(Value::Boolean(false)), Some(num(0.0)), Some(num(1.0)), Some(text("x"))];

        fn graph(rng: &mut Rng, pool: &[(Hash, usize)], n_in: usize) -> Unit {
            let n = 2 + rng.below(8);
            let mut children = Vec::new();
            let mut edges = Vec::new();
            for c in 0..n {
                let (h, k) = pool[rng.below(pool.len())];
                children.push(h);
                for p in 0..k {
                    // From an input, an earlier child, or (rarely) unwired.
                    match rng.below(10) {
                        0 => {}
                        1..=3 => edges.push(edge(BOUNDARY, rng.below(n_in) as u32, c as u32, p as u32)),
                        _ if c > 0 => edges.push(edge(rng.below(c) as u32, 0, c as u32, p as u32)),
                        _ => edges.push(edge(BOUNDARY, rng.below(n_in) as u32, c as u32, p as u32)),
                    }
                }
            }
            let outs = 1 + rng.below(2);
            for o in 0..outs {
                edges.push(edge(rng.below(n) as u32, 0, BOUNDARY, o as u32));
            }
            multi_composite((0..n_in).map(|i| Port { name: format!("in{i}"), ty: Type::Any }).collect(), vec![Type::Any; outs], children, edges)
        }

        let mut r = MapResolver::new();
        let mut pool: Vec<(Hash, usize)> = LEAVES.iter().map(|&(name, k)| (r.insert(prim(name, k, Type::Any)).unwrap(), k)).collect();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        // A few generated holons, single-output, join the pool so graphs nest them.
        while pool.len() < LEAVES.len() + 6 {
            let mut g = graph(&mut rng, &pool, 2);
            g.outputs.truncate(1);
            if let UnitBody::Composite { edges, .. } = &mut g.body {
                edges.retain(|e| !(e.to_child == BOUNDARY && e.to_port > 0));
            }
            // A graph with unused twins (two same-hash children nothing tells apart) has no hash.
            if let Ok(h) = r.insert(g) {
                pool.push((h, 2));
            }
        }
        let mut checked = 0;
        for _ in 0..2000 {
            let g = graph(&mut rng, &pool, 3);
            for _ in 0..4 {
                let inputs: Vec<Option<Value>> = (0..3).map(|_| values[rng.below(values.len())].clone()).collect();
                assert_eq!(eval(&g, &inputs, &r).into_vec(), eval_eager(&g, &inputs, &r), "{g:?} on {inputs:?}");
                checked += 1;
            }
        }
        assert_eq!(checked, 8000);
    }
}
