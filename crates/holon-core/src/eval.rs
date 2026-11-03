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

use crate::primitives::Op;
use crate::{primitives, Edge, Hash, Param, Resolver, Unit, UnitBody, Value, BOUNDARY, VARIADIC};
use crate::hash::parse_hash;
use smallvec::{smallvec, SmallVec};
use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

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
    fn asserted(&mut self, _path: &[u32], _unit: &Unit, _inputs: &[Option<&Value>]) -> Option<Vec<Option<Value>>> {
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
    let inputs: Refs = inputs.iter().map(Option::as_ref).collect();
    eval_at(unit, &inputs, resolver, observer, &mut Vec::new()).into_iter().map(Slot::into_value).collect()
}

/// A node's inputs as the walk hands them over: borrowed from where each is kept (the composite's
/// own inputs, or the output of the child that made it), never copied to be read
/// (docs/L4s/interpreter-path-L4.md, step 3). Inline for the usual few.
type Refs<'a> = SmallVec<[Option<&'a Value>; 4]>;

/// `inputs` owned, for the few nodes whose builders take them so (a request, a module).
fn owned(inputs: &[Option<&Value>]) -> Vec<Option<Value>> {
    inputs.iter().map(|o| o.cloned()).collect()
}

/// What the host answers to a node's request, or absent when the request can't even be built.
fn ask(request: Option<Value>, observer: &mut impl Observer, path: &[u32]) -> Slots {
    smallvec![request.map_or(Slot::Absent, |r| observer.answer(path, &r).into())]
}

fn eval_at(
    unit: &Unit,
    inputs: &[Option<&Value>],
    resolver: &impl Resolver,
    observer: &mut impl Observer,
    path: &mut Vec<u32>,
) -> Slots {
    eval_as(unit, &NodeKind::of(unit), inputs, resolver, observer, path)
}

/// What a node is, as evaluating it needs to know: decided from its unit once, where a composite
/// prepares its children (docs/L4s/interpreter-path-L4.md, step 5's first level), instead of by
/// its name at every evaluation. Per run, not kept: a resolver may hand a run its own unit for a
/// hash (a sourced value's constant).
#[derive(Clone, Copy, Debug, PartialEq)]
struct NodeKind {
    body: Body,
    /// A sink whose last input is its trigger ([`crate::takes_trigger`]).
    takes_trigger: bool,
    /// `guard` or `coalesce`, decided lazily inside a composite ([`choose`]).
    choice: Option<Which>,
    /// Held back while a wired input is absent: a leaf with optional ports, or a sink.
    held_when_absent: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Body {
    Map,
    Fold,
    /// A request the host answers.
    Ask(Request),
    Coalesce,
    /// Any other leaf, computed by `compute` (its [`Op`], none for a name that isn't one); a
    /// variadic one fires only with every input present.
    Leaf { variadic: bool, op: Option<Op> },
    Composite,
    Module,
}

/// The requests a node asks the host, by the leaf that builds them.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Request {
    Http,
    Upload,
    FlowCall,
    Stream,
    Await,
    CollectionGet,
    CollectionList,
    FileRead,
    FileList,
    FileHandle,
    FileLines,
    FileSlice,
    DbQuery,
    DbOne,
    DbFind,
    DbFindOne,
    ResumeToken,
}

impl Request {
    fn of(name: &str) -> Option<Request> {
        Some(match name {
            "http_request" => Request::Http,
            "http_upload" => Request::Upload,
            "flow_call" => Request::FlowCall,
            "http_stream" => Request::Stream,
            "await" => Request::Await,
            "collection_get" => Request::CollectionGet,
            "collection_list" => Request::CollectionList,
            "file_read" => Request::FileRead,
            "file_list" => Request::FileList,
            "file_handle" => Request::FileHandle,
            "file_lines" => Request::FileLines,
            "file_slice" => Request::FileSlice,
            "db_query" => Request::DbQuery,
            "db_one" => Request::DbOne,
            "db_find" => Request::DbFind,
            "db_find_one" => Request::DbFindOne,
            "resume_token" => Request::ResumeToken,
            _ => return None,
        })
    }

    /// The request, built from the node's inputs and params: absent when it can't be.
    fn build(self, inputs: &[Option<&Value>], params: &[Param]) -> Option<Value> {
        match self {
            // An outbound request: answered (or not yet) by the host through the observer.
            Request::Http => primitives::http_request(&owned(inputs), params),
            // One whose body is a file: the host streams it.
            Request::Upload => primitives::http_upload(&owned(inputs), params),
            // A call to another flow's action: the host runs it in-process and answers.
            Request::FlowCall => primitives::flow_call_request(&owned(inputs), params),
            // One whose answer the host forwards to the route's caller as it arrives.
            Request::Stream => primitives::http_stream(&owned(inputs), params),
            // A durable run's waiting point and its token: also the host's to answer.
            Request::Await => primitives::await_request(params),
            Request::ResumeToken => primitives::resume_token_request(params),
            // A collection read: the host answers it from the collection as the run found it.
            Request::CollectionGet => primitives::collection_get_request(&owned(inputs), params),
            Request::CollectionList => primitives::collection_list_request(&owned(inputs), params),
            // A file read: the host answers it from the folder bound to the name, as the run found it.
            Request::FileRead => primitives::file_read_request(&owned(inputs), params),
            Request::FileList => primitives::file_list_request(&owned(inputs), params),
            Request::FileHandle => primitives::file_handle_request(&owned(inputs), params),
            Request::FileLines => primitives::file_lines_request(&owned(inputs), params),
            Request::FileSlice => primitives::file_slice_request(&owned(inputs), params),
            // A database read: the host answers it from the database as the run found it.
            Request::DbQuery => primitives::db_query_request(&owned(inputs), params, false),
            Request::DbOne => primitives::db_query_request(&owned(inputs), params, true),
            Request::DbFind => primitives::db_find_request(&owned(inputs), params, false),
            Request::DbFindOne => primitives::db_find_request(&owned(inputs), params, true),
        }
    }
}

impl NodeKind {
    fn of(unit: &Unit) -> NodeKind {
        let takes_trigger = crate::takes_trigger(unit);
        match &unit.body {
            UnitBody::Primitive { name } => {
                let name = name.as_str();
                let body = match name {
                    // `map` and `fold` are higher-order — they invoke the evaluator — so they live
                    // here, where recursion lives, rather than in first-order `compute`.
                    "map" => Body::Map,
                    "fold" => Body::Fold,
                    "coalesce" => Body::Coalesce,
                    _ => match Request::of(name) {
                        Some(request) => Body::Ask(request),
                        None => Body::Leaf { variadic: VARIADIC.contains(&name), op: Op::of(name) },
                    },
                };
                let choice = match name {
                    "guard" => Some(Which::Guard),
                    "coalesce" => Some(Which::Coalesce),
                    _ => None,
                };
                let held_when_absent = primitives::has_optional_ports(name) || crate::TRIGGERED.contains(&name);
                NodeKind { body, takes_trigger, choice, held_when_absent }
            }
            UnitBody::Composite { .. } => NodeKind { body: Body::Composite, takes_trigger, choice: None, held_when_absent: false },
            UnitBody::Module { .. } => NodeKind { body: Body::Module, takes_trigger, choice: None, held_when_absent: false },
        }
    }
}

/// [`eval_at`], what `unit` is already known (`kind`, [`NodeKind::of`] it).
fn eval_as(
    unit: &Unit,
    kind: &NodeKind,
    inputs: &[Option<&Value>],
    resolver: &impl Resolver,
    observer: &mut impl Observer,
    path: &mut Vec<u32>,
) -> Slots {
    // A sink's trigger only has to be there (the composite holds it back while a wired one is
    // absent): it is dropped here, so the leaf computes on its own inputs as it always did.
    let inputs = if kind.takes_trigger && inputs.len() == unit.inputs.len() { &inputs[..inputs.len() - 1] } else { inputs };
    match (kind.body, &unit.body) {
        (Body::Map, _) => smallvec![eval_map(inputs, &unit.params, resolver, observer, path)],
        // `fold` is the one iteration primitive (the universal list donor). It threads an
        // accumulator, which `map` does not.
        (Body::Fold, _) => smallvec![eval_fold(inputs, &unit.params, resolver, observer, path)],
        (Body::Ask(request), _) => ask(request.build(inputs, &unit.params), observer, path),
        // `coalesce` is the one leaf whose meaning is about absence: the first input that
        // arrived. The generic arm below would give the same answer today (it gathers present
        // inputs in port order), but that is a property of its implementation, not its
        // contract, and the firing rule it serves says the opposite ("a wired input absent,
        // nothing fires"). This arm is the contract, stated once, where absence is visible.
        // (Inside a composite it is a choice, decided by `choice_next` without getting here.)
        (Body::Coalesce, _) => smallvec![Slot::of(inputs.iter().flatten().next().map(|v| (*v).clone()))],
        (Body::Leaf { variadic, op }, UnitBody::Primitive { name }) => {
            // Present values in port order **by reference** — `compute` borrows them, so an O(1)
            // reader (head/length/is_empty) copies only what it returns, not the whole input
            // list. compute's arity check is still the firing rule.
            //
            // That rule alone fails *open* for a variadic leaf: dropping an absent input leaves
            // a shorter list it still accepts, so `and(authenticated, <absent>)` would yield
            // `authenticated` and `list(a, <absent>, c)` would yield `[a, c]`. A variadic leaf
            // therefore fires only when every declared input is present, exactly as a
            // fixed-arity leaf already does.
            if variadic && inputs.iter().any(Option::is_none) {
                return smallvec![Slot::Absent];
            }
            let present: SmallVec<[&Value; 4]> = inputs.iter().flatten().copied().collect();
            smallvec![Slot::of(op.and_then(|op| primitives::compute_op(op, name, &present, &unit.params)))]
        }
        (_, UnitBody::Composite { children, edges }) => eval_composite(unit, children, edges, inputs, resolver, observer, path),
        // A module body: the engine behind the resolver runs it (docs/L4s/wasm-module-L4.md §5);
        // when it can't, the observer hears why, at this node.
        (_, UnitBody::Module { bytes }) => {
            let (outs, reason) = run_module_outputs(bytes, unit, &owned(inputs), resolver);
            if let Some(reason) = reason {
                observer.failed(path, &reason);
            }
            outs.into_iter().map(Slot::of).collect()
        }
        // A primitive whose kind isn't a primitive's: never made so (`NodeKind::of`), but if it
        // were, nothing is computed, as for a leaf that isn't one, instead of stopping the run
        // (docs/L4s/resilience-audit-L4.md, R3).
        (_, UnitBody::Primitive { .. }) => smallvec![Slot::Absent; unit.outputs.len()],
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
    let which = match name {
        "guard" => Which::Guard,
        "coalesce" => Which::Coalesce,
        _ => return None,
    };
    let at = |p: usize| seen.get(p).map_or(Some(In::Absent), |s| s.as_ref().map(In::of_slot));
    Some(match choose(which, at) {
        Chosen::Need(p) => Choice::Need(p),
        Chosen::Done(slot) => Choice::Done(slot.owned()),
    })
}

/// The two choice leaves.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Which {
    Guard,
    Coalesce,
}

/// A slot as the walk reads it: borrowed from where it is kept.
#[derive(Clone, Copy, Debug, PartialEq)]
enum In<'a> {
    Value(&'a Value),
    Absent,
    Pending,
}

impl<'a> In<'a> {
    fn of_slot(slot: &'a Slot) -> In<'a> {
        match slot {
            Slot::Value(v) => In::Value(v),
            Slot::Absent => In::Absent,
            Slot::Pending => In::Pending,
        }
    }
    fn of_value(value: Option<&'a Value>) -> In<'a> {
        value.map_or(In::Absent, In::Value)
    }
    fn value(self) -> Option<&'a Value> {
        match self {
            In::Value(v) => Some(v),
            _ => None,
        }
    }
    fn owned(self) -> Slot {
        match self {
            In::Value(v) => Slot::Value(v.clone()),
            In::Absent => Slot::Absent,
            In::Pending => Slot::Pending,
        }
    }
}

/// [`Choice`], its output borrowed.
enum Chosen<'a> {
    Need(usize),
    Done(In<'a>),
}

/// [`choice_next`]'s rule, over the inputs as `at` gives them (`None`: not yet evaluated).
fn choose<'a>(which: Which, at: impl Fn(usize) -> Option<In<'a>>) -> Chosen<'a> {
    match which {
        Which::Guard => match at(0) {
            None => Chosen::Need(0),
            Some(In::Pending) => Chosen::Done(In::Pending),
            Some(In::Value(Value::Boolean(true))) => match at(1) {
                None => Chosen::Need(1),
                Some(v) => Chosen::Done(v),
            },
            // false, absent, or not a boolean: absent, as `guard` is.
            Some(_) => Chosen::Done(In::Absent),
        },
        Which::Coalesce => match at(0) {
            None => Chosen::Need(0),
            Some(In::Absent) => match at(1) {
                None => Chosen::Need(1),
                Some(v) => Chosen::Done(v),
            },
            Some(first) => Chosen::Done(first),
        },
    }
}

fn eval_composite(
    unit: &Unit,
    children: &[Hash],
    edges: &[Edge],
    inputs: &[Option<&Value>],
    resolver: &impl Resolver,
    observer: &mut impl Observer,
    path: &mut Vec<u32>,
) -> Slots {
    let n_outputs = unit.outputs.len();
    match Prepared::of(unit, children, edges, resolver) {
        Some(prepared) => prepared.run(n_outputs, inputs, resolver, observer, path),
        None => smallvec![Slot::Absent; n_outputs],
    }
}

/// What a composite's walk needs of its edges alone, whatever its inputs and whatever its
/// children resolve to: the edge feeding each child's input, where the walk starts, and whether
/// the edges hold a cycle. It depends on nothing but the unit, so a host that keeps its units
/// keeps it with them ([`Shapes`], [`Resolver::shape`]) instead of making it again for each run
/// (docs/L4s/interpreter-path-L4.md, step 1b).
#[derive(Clone, Debug)]
pub struct Shape {
    /// Destination (to_child, to_port) → source (from_child, from_port).
    incoming: Incoming,
    /// The walk's first stack: what the outputs read, and what nothing consumes.
    roots: Vec<usize>,
    /// The edges hold a cycle: nothing is evaluable.
    cyclic: bool,
}

impl Shape {
    /// The shape of a composite of `n` children joined by `edges`.
    pub fn of(n: usize, edges: &[Edge]) -> Shape {
        // Index incoming edges: destination (to_child, to_port) → source (from_child, from_port).
        // Validation guarantees ≤1 source per destination, so this never loses an edge.
        let incoming = Incoming::of(n, edges);

        // A cycle is impossible after P1.3.1; handled, not panicked, and the walk can then rely
        // on reaching the end of everything it pulls.
        let cyclic = has_cycle(n, edges);

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
        let roots = (0..n).rev().filter(|&c| root[c] || !consumed[c]).collect();
        Shape { incoming, roots, cyclic }
    }

    /// The shape of `unit`, when it is a composite.
    pub fn of_unit(unit: &Unit) -> Option<Shape> {
        match &unit.body {
            UnitBody::Composite { children, edges } => Some(Shape::of(children.len(), edges)),
            _ => None,
        }
    }
}

/// The shapes of a set of units a host keeps, each found by the unit itself: by where it lives,
/// not by its hash, so finding one costs no hashing and a unit a resolver makes up for one run (a
/// sourced constant) never meets a shape that isn't its own.
///
/// Sound as long as the units outlive the table and don't move: an address the table holds is
/// then the address of that very unit, and no other unit can live there. A host builds it over
/// units it holds behind a pointer (an `Arc`, a `Box`) and keeps the two together.
#[derive(Clone, Debug, Default)]
pub struct Shapes(HashMap<usize, Shape, BuildHasherDefault<AddressHasher>>);

impl Shapes {
    /// The shape of each composite among `units`.
    pub fn of<'u>(units: impl IntoIterator<Item = &'u Unit>) -> Shapes {
        let mut shapes = HashMap::default();
        for unit in units {
            if let Some(shape) = Shape::of_unit(unit) {
                shapes.insert(unit as *const Unit as usize, shape);
            }
        }
        Shapes(shapes)
    }

    /// `unit`'s shape, when it is one of the units the table was built over.
    pub fn get(&self, unit: &Unit) -> Option<&Shape> {
        if self.0.is_empty() {
            return None;
        }
        self.0.get(&(unit as *const Unit as usize))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// About what the table takes in memory: its map (a bucket and a half a shape) and each
    /// shape's lists. For a cache that counts what it keeps (holon's flow cache).
    pub fn footprint(&self) -> u64 {
        const ALLOC: u64 = 16;
        let bucket = (std::mem::size_of::<usize>() + std::mem::size_of::<Shape>()) as u64 + 1;
        self.0
            .values()
            .map(|shape| {
                let i = &shape.incoming;
                bucket * 3 / 2
                    + (i.starts.capacity() * 4) as u64
                    + (i.ports.capacity() * std::mem::size_of::<Source>()) as u64
                    + (i.outputs.capacity() * std::mem::size_of::<Option<(u32, u32)>>()) as u64
                    + (shape.roots.capacity() * 8) as u64
                    + 4 * ALLOC
            })
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Hashes an address: units are aligned, so the low bits say nothing and are shifted out.
#[derive(Default)]
struct AddressHasher(u64);

impl Hasher for AddressHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ b as u64;
        }
    }
    fn write_usize(&mut self, n: usize) {
        let n = n as u64;
        self.0 = (n >> 3).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/// What evaluating a composite needs of its unit alone, whatever its inputs: its children
/// resolved, and its [`Shape`]. Made once for a `map`'s or `fold`'s function and run for every
/// item, instead of made again for each (docs/L4s/request-path-L4.md §4.5): it depends on
/// nothing an application changes. Its children are resolved for each run, never kept: a
/// resolver may hand a run its own unit under a hash (a sourced value's constant).
struct Prepared<'r> {
    resolved: Vec<&'r Unit>,
    /// What each child is ([`NodeKind`]), decided once here.
    kinds: Vec<NodeKind>,
    shape: Cow<'r, Shape>,
    /// The walk's scratch, kept for the next run of the same preparation (a `map`'s next item):
    /// emptied after each run, its room kept.
    scratch: std::cell::RefCell<Vec<Option<Slots>>>,
    /// The walk worked out ahead ([`Plan`]), for one run many times over: none, walked.
    plan: Option<Plan>,
}

/// One step of a [`Plan`].
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// Evaluate this child, every child feeding it evaluated already.
    Eval(u32),
    /// This pass-through guard's condition is evaluated: go on only if it is `true`, as it is
    /// made to be; if not (an observer stood in for it), the walk carries on from here.
    Check(u32),
}

/// A composite's walk worked out ahead, for one without choices, whose order no value changes: the
/// order the walk evaluates its children in, and where each child's inputs come from
/// (docs/L4s/interpreter-path-L4.md, step 5, level 2). A run goes down the order, each child's
/// inputs already evaluated, with no stack, no revisit, and nothing to look for.
#[derive(Debug)]
struct Plan {
    order: Vec<Step>,
    /// Each child's inputs, `starts[child]..starts[child + 1]`, one per port it declares.
    starts: Vec<u32>,
    sources: Vec<Source>,
}

impl<'r> Prepared<'r> {
    /// None when a child doesn't resolve or the edges hold a cycle: then nothing is evaluable,
    /// and every declared output slot is absent.
    fn of(unit: &'r Unit, children: &[Hash], edges: &[Edge], resolver: &'r impl Resolver) -> Option<Prepared<'r>> {
        // Resolve children (by hash). Any unresolved ⇒ nothing evaluable — every declared output
        // slot is absent (the vec's *length* is still the declared contract; only the values are
        // absent, mirroring how `inputs` already has a fixed length with individually-absent slots).
        let mut resolved: Vec<&Unit> = Vec::with_capacity(children.len());
        for h in children {
            resolved.push(resolver.resolve(h)?);
        }
        let shape = match resolver.shape(unit) {
            Some(kept) => Cow::Borrowed(kept),
            None => Cow::Owned(Shape::of(children.len(), edges)),
        };
        if shape.cyclic {
            return None;
        }
        let kinds = resolved.iter().map(|u| NodeKind::of(u)).collect();
        Some(Prepared { resolved, kinds, shape, scratch: std::cell::RefCell::new(Vec::new()), plan: None })
    }

    /// Evaluate it on `inputs`.
    fn run(&self, n_outputs: usize, inputs: &[Option<&Value>], resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slots {
        // The scratch kept from the last run, or a fresh one if a run of this very preparation is
        // somehow under way.
        let mut kept = self.scratch.try_borrow_mut().ok();
        let mut fresh = Vec::new();
        let scratch: &mut Vec<Option<Slots>> = match kept.as_deref_mut() {
            Some(kept) => kept,
            None => &mut fresh,
        };
        let outs = match &self.plan {
            Some(plan) => self.run_planned(plan, scratch, n_outputs, inputs, resolver, observer, path),
            None => self.run_in(scratch, n_outputs, inputs, resolver, observer, path),
        };
        scratch.clear();
        outs
    }

    /// It, with its walk worked out ahead ([`Plan`]) when it has no choice: for a preparation
    /// run many times (a `map`'s or `fold`'s function).
    fn planned(mut self) -> Prepared<'r> {
        if (0..self.kinds.len()).all(|ci| self.kinds[ci].choice.is_none() || self.passes(ci)) {
            self.plan = Some(self.plan_of());
        }
        self
    }

    /// Whether child `ci` is a pass-through: a `guard` whose condition is a `const` `true` (how
    /// the language shares one input among several nodes). Its order is then the walk's whatever
    /// its value, as long as the condition reads `true`, which the plan checks.
    fn passes(&self, ci: usize) -> bool {
        self.kinds[ci].choice == Some(Which::Guard)
            && match self.shape.incoming.source(ci, 0) {
                Source::Child(c, 0) => {
                    let cond = self.resolved[c as usize];
                    matches!(&cond.body, UnitBody::Primitive { name } if name == "const")
                        && cond.inputs.is_empty()
                        && cond.params.iter().any(|p| p.name() == "type" && p.value() == Some("Boolean"))
                        && cond.params.iter().any(|p| p.name() == "value" && p.value() == Some("true"))
                }
                _ => false,
            }
    }

    /// The order the walk ([`run_in`](Prepared::run_in)) evaluates its children in, worked out by
    /// its own rule without evaluating anything: from the roots, depth first, a child once every
    /// child feeding it is, its inputs pulled in port order. Without a choice nothing a value
    /// says changes it.
    fn plan_of(&self) -> Plan {
        let incoming = &self.shape.incoming;
        let n = self.resolved.len();
        let mut done = vec![false; n];
        let mut checked = vec![false; n];
        let mut order = Vec::with_capacity(n);
        let mut stack: Vec<usize> = self.shape.roots.clone();
        while let Some(&ci) = stack.last() {
            if done[ci] {
                stack.pop();
                continue;
            }
            let k = self.resolved[ci].inputs.len();
            let unevaluated = |p: usize| match incoming.source(ci, p) {
                Source::Child(fc, _) if !done[fc as usize] => Some(fc as usize),
                _ => None,
            };
            // A pass-through guard's condition is checked once it is in, before its value is
            // pulled: where the walk's choice reads it.
            if self.kinds[ci].choice.is_some() && !checked[ci] && unevaluated(0).is_none() {
                checked[ci] = true;
                order.push(Step::Check(ci as u32));
            }
            if let Some(fc) = (0..k).find_map(unevaluated) {
                stack.push(fc);
                continue;
            }
            stack.pop();
            done[ci] = true;
            order.push(Step::Eval(ci as u32));
        }
        let mut starts = Vec::with_capacity(n + 1);
        let mut sources = Vec::new();
        starts.push(0);
        for ci in 0..n {
            sources.extend((0..self.resolved[ci].inputs.len()).map(|p| incoming.source(ci, p)));
            starts.push(sources.len() as u32);
        }
        Plan { order, starts, sources }
    }

    /// [`run_in`](Prepared::run_in), down its [`Plan`].
    #[allow(clippy::too_many_arguments)]
    fn run_planned(&self, plan: &Plan, scratch: &mut Vec<Option<Slots>>, n_outputs: usize, inputs: &[Option<&Value>], resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slots {
        scratch.resize(self.resolved.len(), None);
        let open = crate::open_trigger();
        for &step in &plan.order {
            let ci = match step {
                Step::Eval(ci) => ci as usize,
                Step::Check(g) => {
                    let cond = match self.shape.incoming.source(g as usize, 0) {
                        Source::Child(c, 0) => scratch[c as usize].as_ref().and_then(|outs| outs.first()),
                        _ => None,
                    };
                    if cond != Some(&Slot::Value(Value::Boolean(true))) {
                        // Not as made: the walk goes on from what is evaluated, which is where it
                        // would be.
                        return self.run_in(scratch, n_outputs, inputs, resolver, observer, path);
                    }
                    continue;
                }
            };
            if self.kinds[ci].choice.is_some() {
                // A pass-through guard, its condition `true`: its value, as the walk's choice
                // gives it. Its condition and its value are in by the plan's order; if either
                // weren't, the walk carries on from here (R3).
                let cond = match self.shape.incoming.source(ci, 0) {
                    Source::Child(c, 0) => scratch[c as usize].as_ref().and_then(|outs| outs.first()),
                    _ => None,
                };
                let value = match self.shape.incoming.source(ci, 1) {
                    Source::Input(fp) => Some(Slot::of(inputs.get(fp as usize).copied().flatten().cloned())),
                    Source::Child(fc, fp) => scratch[fc as usize].as_ref().map(|o| o.get(fp as usize).cloned().unwrap_or(Slot::Absent)),
                    Source::Unwired | Source::Missing(..) => Some(Slot::Absent),
                };
                let (Some(Slot::Value(Value::Boolean(true))), Some(value)) = (cond, value) else {
                    return self.run_in(scratch, n_outputs, inputs, resolver, observer, path);
                };
                let outs: Slots = smallvec![value];
                path.push(ci as u32);
                observer.enter(path);
                observer.produced(path, &outs);
                observer.leave(path);
                path.pop();
                scratch[ci] = Some(outs);
                continue;
            }
            let outs = {
                let mut gathered: Refs = SmallVec::new();
                let mut pending = false;
                let mut ahead = false;
                for source in &plan.sources[plan.starts[ci] as usize..plan.starts[ci + 1] as usize] {
                    let slot = match *source {
                        Source::Input(fp) => In::of_value(inputs.get(fp as usize).copied().flatten()),
                        Source::Child(fc, fp) => match &scratch[fc as usize] {
                            Some(outs) => outs.get(fp as usize).map_or(In::Absent, In::of_slot),
                            // Never so (the plan is the walk's order), but if it were, the walk
                            // carries on from here, as it does after a failed check (R3).
                            None => {
                                ahead = true;
                                break;
                            }
                        },
                        Source::Unwired | Source::Missing(..) => In::Absent,
                    };
                    pending |= slot == In::Pending;
                    gathered.push(slot.value());
                }
                (!ahead).then(|| self.fire(ci, &mut gathered, pending, &open, resolver, observer, path))
            };
            let Some(outs) = outs else {
                return self.run_in(scratch, n_outputs, inputs, resolver, observer, path);
            };
            scratch[ci] = Some(outs);
        }
        self.outputs(scratch, n_outputs, inputs)
    }

    /// Evaluate child `ci` on its inputs (`gathered`, every one evaluated; `pending` when one is
    /// still waiting on an answer), telling the observer: what the walk and the plan do with a
    /// child once its inputs are in.
    #[allow(clippy::too_many_arguments)]
    fn fire<'v>(&self, ci: usize, gathered: &mut Refs<'v>, pending: bool, open: &'v Value, resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slots {
        let (child, kind, incoming) = (self.resolved[ci], &self.kinds[ci], &self.shape.incoming);
        let k = child.inputs.len();
        if pending {
            // Waiting on an answer: not run in this round.
            return smallvec![Slot::Pending; child.outputs.len()];
        }
        // A leaf with optional ports (an `http_request`'s headers and body) reads an *unwired*
        // one as "none"; a *wired* one whose value is absent means what absence means everywhere
        // else: the node doesn't fire. Otherwise a request downstream of a check that failed (a
        // webhook whose signature didn't verify) still went out, bodiless.
        open_triggers(child, gathered, || open, |p| incoming.wired(ci as u32, p as u32));
        // A sink is held the same way: its trigger, wired, hasn't arrived.
        let held = kind.held_when_absent && (0..k).any(|p| gathered[p].is_none() && incoming.wired(ci as u32, p as u32));
        path.push(ci as u32);
        observer.enter(path);
        let outs = if held {
            smallvec![Slot::Absent; child.outputs.len()]
        } else {
            match observer.asserted(path, child, gathered) {
                Some(asserted) if asserted.len() == child.outputs.len() => asserted.into_iter().map(Slot::of).collect(),
                _ => eval_as(child, kind, gathered, resolver, observer, path),
            }
        };
        observer.produced(path, &outs);
        observer.leave(path);
        path.pop();
        outs
    }

    /// [`run`](Prepared::run), in `scratch` (empty).
    fn run_in(&self, scratch: &mut Vec<Option<Slots>>, n_outputs: usize, inputs: &[Option<&Value>], resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slots {
        let (resolved, incoming) = (&self.resolved, &self.shape.incoming);
        let n = resolved.len();
        let mut stack: SmallVec<[usize; 8]> = self.shape.roots.iter().copied().collect();

        // Per-evaluation scratch, indexed by child: `None` not evaluated (yet, or at all), else
        // EACH child's own output slots (a child may itself be a multi-output composite).
        scratch.resize(n, None);

        // The slot feeding a child's input port, borrowed where it is kept: `None` while its
        // source child is unevaluated.
        fn slot_in<'s>(incoming: &Incoming, inputs: &[Option<&'s Value>], scratch: &'s [Option<Slots>], ci: usize, p: usize) -> Option<In<'s>> {
            match incoming.source(ci, p) {
                Source::Input(fp) => Some(In::of_value(inputs.get(fp as usize).copied().flatten())),
                Source::Child(fc, fp) => scratch[fc as usize].as_ref().map(|outs| outs.get(fp as usize).map_or(In::Absent, In::of_slot)),
                Source::Unwired | Source::Missing(..) => Some(In::Absent),
            }
        }

        // What a placed holon's open trigger reads as (`open_triggers`).
        let open = crate::open_trigger();

        // Depth-first from the roots, with an explicit stack: a node is run once everything it
        // needs is evaluated; until then, what it needs next is pushed. Looking at a node copies
        // nothing, so one revisited while it waits costs only the look.
        while let Some(&ci) = stack.last() {
            if scratch[ci].is_some() {
                stack.pop();
                continue;
            }
            let (child, kind) = (resolved[ci], &self.kinds[ci]);
            let k = child.inputs.len();
            let outs: Slots = {
                let at = |p: usize| slot_in(incoming, inputs, scratch, ci, p);
                // A choice pulls what it needs; anything else, every input not yet evaluated. Its
                // inputs are gathered in the same pass that looks for one not yet evaluated.
                let mut gathered: Refs = SmallVec::new();
                let mut pending = false;
                let need = match kind.choice.map(|which| choose(which, at)) {
                    Some(Chosen::Need(p)) => Some(p),
                    Some(Chosen::Done(slot)) => {
                        stack.pop();
                        path.push(ci as u32);
                        observer.enter(path);
                        let outs: Slots = smallvec![slot.owned()];
                        observer.produced(path, &outs);
                        observer.leave(path);
                        path.pop();
                        drop(gathered);
                        scratch[ci] = Some(outs);
                        continue;
                    }
                    None => (0..k).find(|&p| match at(p) {
                        None => true,
                        Some(slot) => {
                            pending |= slot == In::Pending;
                            gathered.push(slot.value());
                            false
                        }
                    }),
                };
                if let Some(p) = need {
                    let (fc, _) = incoming.get(ci as u32, p as u32).expect("only a wired port is unevaluated");
                    stack.push(fc as usize);
                    continue;
                }
                stack.pop();
                self.fire(ci, &mut gathered, pending, &open, resolver, observer, path)
            };
            scratch[ci] = Some(outs);
        }

        self.outputs(scratch, n_outputs, inputs)
    }

    /// The composite's outputs, from the children's slots in `scratch` (moved out where only one
    /// output reads one) or its own inputs.
    fn outputs(&self, scratch: &mut [Option<Slots>], n_outputs: usize, inputs: &[Option<&Value>]) -> Slots {
        let incoming = &self.shape.incoming;
        // Each exposed output is whatever feeds that boundary-output port: a child's output at the
        // right port, or — symmetric with the child-input case above — a promoted input routed
        // straight through. (A composite whose output *is* one of its inputs is a valid,
        // validate-accepted shape; the BOUNDARY→BOUNDARY arm below is why that doesn't panic.)
        // The scratch goes when this returns, so a child's output only one exposed output reads is
        // moved out of it, not copied.
        let source = |op: u32| incoming.get(BOUNDARY, op);
        (0..n_outputs as u32)
            .map(|op| match source(op) {
                Some((BOUNDARY, fp)) => Slot::of(inputs.get(fp as usize).copied().flatten().cloned()),
                Some((fc, fp)) => {
                    let shared = (0..n_outputs as u32).any(|other| other != op && source(other) == Some((fc, fp)));
                    let kept = scratch.get_mut(fc as usize).and_then(|o| o.as_mut()).and_then(|o| o.get_mut(fp as usize));
                    match kept {
                        Some(slot) if !shared => std::mem::replace(slot, Slot::Absent),
                        Some(slot) => slot.clone(),
                        None => Slot::Absent,
                    }
                }
                None => Slot::Absent,
            })
            .collect()
    }
}

/// A composite's edges by destination: for each child's input port, and each boundary output, the
/// (child, port) that feeds it, if one does. Lists indexed by child and port, not a map: looked
/// up for every port of every node a run evaluates.
#[derive(Clone, Debug)]
struct Incoming {
    /// Each child's ports, from `starts[child]` to `starts[child + 1]`, in one list.
    starts: Vec<u32>,
    ports: Vec<Source>,
    outputs: Vec<Option<(u32, u32)>>,
}

/// What feeds one child's input port, worked out when the index is made.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source {
    /// Nothing is wired to it.
    Unwired,
    /// The composite's own input at this port.
    Input(u32),
    /// This child's output at this port.
    Child(u32, u32),
    /// An edge from a child that doesn't exist (bytes that never went through `validate`): read
    /// as absent.
    Missing(u32, u32),
}

impl Incoming {
    fn of(n: usize, edges: &[Edge]) -> Incoming {
        // How many ports each child's list holds: up to the highest one an edge feeds.
        let mut widths = vec![0u32; n];
        let mut outputs: Vec<Option<(u32, u32)>> = Vec::new();
        for e in edges {
            if e.to_child == BOUNDARY {
                let p = e.to_port as usize;
                if outputs.len() <= p {
                    outputs.resize(p + 1, None);
                }
            } else if let Some(w) = widths.get_mut(e.to_child as usize) {
                *w = (*w).max(e.to_port + 1);
            }
            // A destination that doesn't exist: nothing ever looks it up.
        }
        let mut starts = Vec::with_capacity(n + 1);
        starts.push(0u32);
        for w in &widths {
            starts.push(starts.last().unwrap() + w);
        }
        let mut ports = vec![Source::Unwired; *starts.last().unwrap() as usize];
        // The last edge to a destination wins, as the map this replaced kept it.
        for e in edges {
            if e.to_child == BOUNDARY {
                outputs[e.to_port as usize] = Some((e.from_child, e.from_port));
            } else if (e.to_child as usize) < n {
                ports[(starts[e.to_child as usize] + e.to_port) as usize] = match e.from_child {
                    BOUNDARY => Source::Input(e.from_port),
                    fc if (fc as usize) < n => Source::Child(fc, e.from_port),
                    fc => Source::Missing(fc, e.from_port),
                };
            }
        }
        Incoming { starts, ports, outputs }
    }

    /// What feeds port `port` of child `child`.
    fn source(&self, child: usize, port: usize) -> Source {
        let (start, end) = (self.starts[child] as usize, self.starts[child + 1] as usize);
        if port < end - start { self.ports[start + port] } else { Source::Unwired }
    }

    /// What feeds port `port` of `child` (`BOUNDARY` for an output of the composite).
    fn get(&self, child: u32, port: u32) -> Option<(u32, u32)> {
        if child == BOUNDARY {
            return self.outputs.get(port as usize).copied().flatten();
        }
        if child as usize >= self.starts.len().saturating_sub(1) {
            return None;
        }
        match self.source(child as usize, port as usize) {
            Source::Unwired => None,
            Source::Input(fp) => Some((BOUNDARY, fp)),
            Source::Child(fc, fp) | Source::Missing(fc, fp) => Some((fc, fp)),
        }
    }

    fn wired(&self, child: u32, port: u32) -> bool {
        self.get(child, port).is_some()
    }
}

/// Whether the in-range child→child edges hold a cycle: exactly when [`topo_order`] has no order,
/// worked out with lists alone, without the order it would make (Kahn's count of what can be
/// reached from the nodes nothing feeds).
fn has_cycle(n: usize, edges: &[Edge]) -> bool {
    let inner = || edges.iter().filter(|e| (e.from_child as usize) < n && (e.to_child as usize) < n);
    let mut indeg = vec![0u32; n];
    let mut start = vec![0usize; n + 1];
    for e in inner() {
        indeg[e.to_child as usize] += 1;
        start[e.from_child as usize + 1] += 1;
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut next = start.clone();
    let mut adj = vec![0usize; start[n]];
    for e in inner() {
        let f = e.from_child as usize;
        adj[next[f]] = e.to_child as usize;
        next[f] += 1;
    }
    let mut ready: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut seen = 0;
    while let Some(node) = ready.pop() {
        seen += 1;
        for &m in &adj[start[node]..start[node + 1]] {
            indeg[m] -= 1;
            if indeg[m] == 0 {
                ready.push(m);
            }
        }
    }
    seen < n
}

/// A placed holon's trigger left unwired reads as present ([`crate::open_trigger`]): its sinks run
/// with the unit around it, as a leaf's open trigger lets it.
pub(crate) fn open_triggers<T>(child: &Unit, gathered: &mut [Option<T>], open: impl Fn() -> T, wired: impl Fn(usize) -> bool) {
    if matches!(child.body, UnitBody::Primitive { .. }) {
        return;
    }
    for (p, port) in child.inputs.iter().enumerate() {
        if port.name == crate::TRIGGER && gathered.get(p).is_some_and(Option::is_none) && !wired(p) {
            gathered[p] = Some(open());
        }
    }
}

/// Kahn's algorithm over in-range child→child edges, emitting a deterministic order
/// (ascending child index among ready nodes). Boundary edges are excluded (an endpoint
/// `== BOUNDARY` is not `< n`). Returns `None` on a cycle — impossible after P1.3.1, but
/// handled rather than panicking.
#[cfg(any(test, feature = "eager-oracle"))]
pub(crate) fn topo_order(n: usize, edges: &[Edge]) -> Option<Vec<usize>> {
    use std::collections::BTreeSet;
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
fn eval_map(inputs: &[Option<&Value>], params: &[Param], resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slot {
    let list = match inputs.first() {
        Some(Some(Value::List(xs))) => xs,
        _ => return Slot::Absent,
    };
    let Some(extra) = present(&inputs[1..]) else { return Slot::Absent };
    let Some(unit) = function(params, resolver) else { return Slot::Absent };
    let unit = Ready::of(unit, resolver);

    let mut out = Vec::with_capacity(list.len());
    let (mut pending, mut absent) = (false, false);
    let mut asks = Asks(observer);
    // The item and the extras, lent: an application copies only what it keeps. Made once, the
    // item put in place for each.
    let mut args: Refs = SmallVec::with_capacity(1 + extra.len());
    args.push(None);
    args.extend_from_slice(extra);
    for (k, elem) in list.iter().enumerate() {
        args[0] = Some(elem);
        // Every item is applied, even after one comes out absent, so each item's requests are
        // asked in the same round rather than one round per item.
        match apply(&unit, &args, resolver, &mut asks, path, k) {
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

/// Whether `unit`, applied by a `map` or `fold`, runs down a plan worked out ahead (step 5, level 2)
/// rather than walked: a composite whose only choices are pass-throughs. For a test that a flow's
/// functions keep their plans when the language changes (docs/L4s/resilience-audit-L4.md, D5).
#[doc(hidden)]
pub fn would_plan(unit: &Unit, resolver: &impl Resolver) -> bool {
    match &unit.body {
        UnitBody::Composite { children, edges } => Prepared::of(unit, children, edges, resolver).is_some_and(|p| p.planned().plan.is_some()),
        _ => false,
    }
}

/// A `map`'s or `fold`'s function, ready to apply to each item: a composite prepared once
/// ([`Prepared`]), so each application only runs it.
enum Ready<'r> {
    /// A composite: its unit, and it prepared; or None, when it can't be (each application then
    /// gives nothing, as `eval_composite` would).
    Composite(&'r Unit, Option<Box<Prepared<'r>>>),
    /// Anything else, evaluated as it always is.
    Other(&'r Unit),
}

impl<'r> Ready<'r> {
    fn of(unit: &'r Unit, resolver: &'r impl Resolver) -> Ready<'r> {
        match &unit.body {
            UnitBody::Composite { children, edges } => Ready::Composite(unit, Prepared::of(unit, children, edges, resolver).map(|p| Box::new(p.planned()))),
            _ => Ready::Other(unit),
        }
    }
}

/// One application of a `map`/`fold` function, to item `k`: evaluated at the node's path then
/// `k`, its one output (or absent, as [`eval_one`]). A composite runs as `eval_at` would run it.
fn apply(unit: &Ready, args: &[Option<&Value>], resolver: &impl Resolver, asks: &mut Asks, path: &mut Vec<u32>, k: usize) -> Slot {
    let Ok(k) = u32::try_from(k) else { return Slot::Absent };
    path.push(k);
    let mut outs = match unit {
        Ready::Other(unit) => eval_at(unit, args, resolver, asks, path),
        Ready::Composite(unit, prepared) => {
            // As `eval_at`: a sink's trigger only has to be there.
            let args = if crate::takes_trigger(unit) && args.len() == unit.inputs.len() { &args[..args.len() - 1] } else { args };
            match prepared {
                Some(prepared) => prepared.run(unit.outputs.len(), args, resolver, asks, path),
                None => smallvec![Slot::Absent; unit.outputs.len()],
            }
        }
    };
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
fn present<'a, 'v>(inputs: &'a [Option<&'v Value>]) -> Option<&'a [Option<&'v Value>]> {
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
fn eval_fold(inputs: &[Option<&Value>], params: &[Param], resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slot {
    let init = match inputs.first() {
        Some(Some(v)) => (*v).clone(),
        _ => return Slot::Absent,
    };
    let list = match inputs.get(1) {
        Some(Some(Value::List(xs))) => xs,
        _ => return Slot::Absent,
    };
    let Some(extra) = present(inputs.get(2..).unwrap_or_default()) else { return Slot::Absent };
    let Some(unit) = function(params, resolver) else { return Slot::Absent };
    let unit = Ready::of(unit, resolver);

    let mut acc = init;
    let mut asks = Asks(observer);
    for (k, elem) in list.iter().enumerate() {
        // fn(acc, elem, extra…) → new acc; absent or pending ⇒ the whole fold is. All lent.
        let next = {
            let mut args: Refs = SmallVec::with_capacity(2 + extra.len());
            args.push(Some(&acc));
            args.push(Some(elem));
            args.extend_from_slice(extra);
            apply(&unit, &args, resolver, &mut asks, path, k)
        };
        acc = match next {
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

    /// The cycle check the evaluator uses agrees with `topo_order` (the eager oracle's) on every
    /// graph: random ones, with and without cycles, and edges to the boundary or past the
    /// composite's children.
    #[test]
    fn has_cycle_is_exactly_where_topo_order_finds_no_order() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        let (mut cyclic, mut acyclic) = (0, 0);
        for _ in 0..4000 {
            let n = next(8) as usize;
            let edges: Vec<Edge> = (0..next(14))
                .map(|_| {
                    let end = |r: u64| if r == 0 { BOUNDARY } else { (r - 1) as u32 };
                    edge(end(next(n as u64 + 3)), next(3) as u32, end(next(n as u64 + 3)), next(3) as u32)
                })
                .collect();
            let found = has_cycle(n, &edges);
            assert_eq!(found, topo_order(n, &edges).is_none(), "n {n}, edges {edges:?}");
            if found { cyclic += 1 } else { acyclic += 1 }
        }
        assert!(cyclic > 100 && acyclic > 100, "both kinds tried: {cyclic} cyclic, {acyclic} not");
    }

    /// The flat index gives, for every child's port and every output, what a map of the edges by
    /// destination gives (the last edge to one winning), edges naming children that don't exist
    /// included.
    #[test]
    fn the_flat_index_finds_what_a_map_of_the_edges_finds() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        for _ in 0..4000 {
            let n = next(8) as usize;
            let end = |r: u64| if r == 0 { BOUNDARY } else { (r - 1) as u32 };
            let edges: Vec<Edge> = (0..next(16)).map(|_| edge(end(next(n as u64 + 3)), next(4) as u32, end(next(n as u64 + 3)), next(4) as u32)).collect();
            let mut by_destination = BTreeMap::new();
            for e in &edges {
                by_destination.insert((e.to_child, e.to_port), (e.from_child, e.from_port));
            }
            let index = Incoming::of(n, &edges);
            for child in (0..n as u32 + 2).chain([BOUNDARY]) {
                for port in 0..6 {
                    // A destination past the children is never looked up by the walk; the map
                    // would hold it, the index needn't.
                    let wanted = if child != BOUNDARY && child as usize >= n { None } else { by_destination.get(&(child, port)).copied() };
                    assert_eq!(index.get(child, port), wanted, "child {child} port {port}, n {n}, edges {edges:?}");
                }
            }
        }
    }

    /// Events as an observer hears them, standing in `false` for the outputs of the children at
    /// `asserted` (as `holon verify` stands in recorded outputs).
    #[derive(Default)]
    struct Events(Vec<String>, Vec<Vec<u32>>);
    impl Observer for Events {
        fn enter(&mut self, path: &[u32]) {
            self.0.push(format!("enter {path:?}"));
        }
        fn produced(&mut self, path: &[u32], outputs: &[Slot]) {
            self.0.push(format!("produced {path:?} {outputs:?}"));
        }
        fn leave(&mut self, path: &[u32]) {
            self.0.push(format!("leave {path:?}"));
        }
        fn asserted(&mut self, path: &[u32], _unit: &Unit, _inputs: &[Option<&Value>]) -> Option<Vec<Option<Value>>> {
            self.1.iter().any(|p| p == path).then(|| vec![Some(Value::Boolean(false))])
        }
    }

    /// Random composites of leaves for the plan's tests: some inputs fed nothing, some fed by a
    /// child that doesn't exist, some dividing by zero (absent, which then spreads), and some
    /// shared through a pass-through guard (a `guard` on a `const` `true`, as the language
    /// shares an input). Each: the unit, its children, its edges, its outputs' count, and the
    /// children that are a guard's condition.
    fn random_composites(r: &mut MapResolver, count: usize, mut each: impl FnMut(&Unit, &[Hash], &[Edge], usize, &[u32])) {
        let leaves = [
            r.insert(prim("subtract", 2, Type::Number)).unwrap(),
            r.insert(prim("add", 2, Type::Number)).unwrap(),
            r.insert(prim("divide", 2, Type::Number)).unwrap(),
            r.insert(prim("guard", 2, Type::Number)).unwrap(),
        ];
        let truth = r
            .insert(Unit {
                body: UnitBody::Primitive { name: "const".into() },
                params: vec![Param::frozen("type", "Boolean"), Param::frozen("value", "true")],
                inputs: vec![],
                outputs: vec![Port { name: "out".into(), ty: Type::Boolean }],
            })
            .unwrap();
        let mut seed = 0x51_7cc1_b727_220au64;
        let mut next = move |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m.max(1)
        };
        for _ in 0..count {
            let n = 1 + next(8) as usize;
            let mut children: Vec<Hash> = (0..n).map(|_| leaves[next(4) as usize]).collect();
            // A random order: a child takes inputs only from children before it in it.
            let mut rank: Vec<usize> = (0..n).collect();
            for i in (1..n).rev() {
                rank.swap(i, next(i as u64 + 1) as usize);
            }
            let mut edges = Vec::new();
            let mut conditions = Vec::new();
            for c in 0..n {
                for port in 0..2 {
                    if children[c] == leaves[3] && port == 0 {
                        // A guard's condition: its own `true`.
                        children.push(truth);
                        conditions.push(children.len() as u32 - 1);
                        edges.push(edge(children.len() as u32 - 1, 0, c as u32, 0));
                        continue;
                    }
                    let earlier: Vec<usize> = (0..n).filter(|&o| rank[o] < rank[c]).collect();
                    match next(6) {
                        0 => {}
                        1 => edges.push(edge(BOUNDARY, next(2) as u32, c as u32, port)),
                        2 => edges.push(edge(40, 0, c as u32, port)),
                        _ if !earlier.is_empty() => edges.push(edge(earlier[next(earlier.len() as u64) as usize] as u32, 0, c as u32, port)),
                        _ => edges.push(edge(BOUNDARY, 0, c as u32, port)),
                    }
                }
            }
            let n_out = 1 + next(2) as usize;
            for op in 0..n_out {
                let from = if next(5) == 0 { BOUNDARY } else { next(n as u64) as u32 };
                edges.push(edge(from, 0, BOUNDARY, op as u32));
            }
            let unit = multi_composite(vec![nport("a"), nport("b")], vec![Type::Number; n_out], children.clone(), edges.clone());
            each(&unit, &children, &edges, n_out, &conditions);
        }
    }

    /// Run `unit` walked and down its plan (twice, the second in the scratch the first left),
    /// with each observer `observer` makes: the same events, in the same order, and outputs.
    fn walk_and_plan_agree(unit: &Unit, children: &[Hash], edges: &[Edge], n_out: usize, r: &MapResolver, observer: impl Fn() -> Events) {
        let inputs = [Some(num(3.0)), Some(num(0.0))];
        let refs: Refs = inputs.iter().map(Option::as_ref).collect();
        let walk = Prepared::of(unit, children, edges, r).unwrap();
        let (mut walked, mut walked_path) = (observer(), Vec::new());
        let walked_out = walk.run(n_out, &refs, r, &mut walked, &mut walked_path);
        let plan = Prepared::of(unit, children, edges, r).unwrap().planned();
        assert!(plan.plan.is_some(), "no choice but pass-throughs: it has a plan");
        for _ in 0..2 {
            let (mut ran, mut ran_path) = (observer(), Vec::new());
            let ran_out = plan.run(n_out, &refs, r, &mut ran, &mut ran_path);
            assert_eq!(ran.0, walked.0, "events, edges {edges:?}");
            assert_eq!(ran_out, walked_out, "outputs, edges {edges:?}");
        }
    }

    /// A composite without choices but pass-throughs, run down its plan, does what the walk does:
    /// the same events in the same order (each child entered, its outputs, left) and the same
    /// outputs, on 3,000 random graphs.
    #[test]
    fn a_composite_run_down_its_plan_does_what_the_walk_does() {
        let mut r = MapResolver::new();
        let mut cases = Vec::new();
        random_composites(&mut r, 3000, |u, c, e, n, _| cases.push((u.clone(), c.to_vec(), e.to_vec(), n)));
        let mut guarded = 0;
        for (unit, children, edges, n_out) in &cases {
            guarded += children.iter().filter(|h| r.resolve(h).is_some_and(|u| matches!(&u.body, UnitBody::Primitive { name } if name == "guard"))).count().min(1);
            walk_and_plan_agree(unit, children, edges, *n_out, &r, Events::default);
        }
        assert!(guarded > 1000, "pass-throughs tried: {guarded}");
    }

    /// A pass-through whose condition an observer stands in `false` for: the plan's check finds
    /// it isn't `true`, the walk carries on from there, and the run is the walk's, event for event.
    #[test]
    fn a_pass_through_found_not_true_hands_the_run_to_the_walk() {
        let mut r = MapResolver::new();
        let mut cases = Vec::new();
        random_composites(&mut r, 3000, |u, c, e, n, conditions| {
            if let Some(&first) = conditions.first() {
                cases.push((u.clone(), c.to_vec(), e.to_vec(), n, first));
            }
        });
        assert!(cases.len() > 1000, "{} cases", cases.len());
        for (unit, children, edges, n_out, condition) in &cases {
            walk_and_plan_agree(unit, children, edges, *n_out, &r, || Events(Vec::new(), vec![vec![*condition]]));
        }
    }

    /// A plan whose order is wrong (here turned around, so a child comes before what feeds it) never
    /// stops the run: where it finds an input not yet evaluated it hands the run to the walk, and
    /// the outputs are the walk's (R3). A node whose kind isn't its body's computes nothing.
    #[test]
    fn a_plan_out_of_order_hands_over_and_a_kind_that_isnt_its_bodys_computes_nothing() {
        let mut r = MapResolver::new();
        let mut cases = Vec::new();
        random_composites(&mut r, 1000, |u, c, e, n, _| cases.push((u.clone(), c.to_vec(), e.to_vec(), n)));
        let inputs = [Some(num(3.0)), Some(num(0.0))];
        let refs: Refs = inputs.iter().map(Option::as_ref).collect();
        let mut handed = 0;
        for (unit, children, edges, n_out) in &cases {
            let walked = Prepared::of(unit, children, edges, &r).unwrap().run(*n_out, &refs, &r, &mut NoObserver, &mut Vec::new());
            let mut plan = Prepared::of(unit, children, edges, &r).unwrap().planned();
            let order = &mut plan.plan.as_mut().unwrap().order;
            let before = order.clone();
            order.reverse();
            handed += usize::from(*order != before);
            assert_eq!(plan.run(*n_out, &refs, &r, &mut NoObserver, &mut Vec::new()), walked, "edges {edges:?}");
        }
        assert!(handed > 500, "{handed} plans turned around");
        let add = prim("add", 2, Type::Number);
        let wrong = NodeKind { body: Body::Composite, takes_trigger: false, choice: None, held_when_absent: false };
        let (a, b) = (num(1.0), num(2.0));
        assert_eq!(eval_as(&add, &wrong, &[Some(&a), Some(&b)], &r, &mut NoObserver, &mut Vec::new()).into_vec(), vec![Slot::Absent]);
    }

    /// A run while the scratch is taken (a run of the same preparation under way, which nothing
    /// makes) goes on in a scratch of its own, with the same outputs (S4).
    #[test]
    fn a_run_whose_scratch_is_taken_uses_one_of_its_own() {
        let mut r = MapResolver::new();
        let mut cases = Vec::new();
        random_composites(&mut r, 200, |u, c, e, n, _| cases.push((u.clone(), c.to_vec(), e.to_vec(), n)));
        let inputs = [Some(num(3.0)), Some(num(0.0))];
        let refs: Refs = inputs.iter().map(Option::as_ref).collect();
        for (unit, children, edges, n_out) in &cases {
            for prepared in [Prepared::of(unit, children, edges, &r).unwrap(), Prepared::of(unit, children, edges, &r).unwrap().planned()] {
                let free = prepared.run(*n_out, &refs, &r, &mut NoObserver, &mut Vec::new());
                let _taken = prepared.scratch.borrow_mut();
                assert_eq!(prepared.run(*n_out, &refs, &r, &mut NoObserver, &mut Vec::new()), free, "edges {edges:?}");
            }
        }
    }

    /// A composite with a choice has no plan: what it evaluates depends on a value, so it is walked.
    #[test]
    fn a_composite_with_a_choice_is_walked() {
        let mut r = MapResolver::new();
        let guard = r.insert(prim("guard", 2, Type::Number)).unwrap();
        let edges = vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, BOUNDARY, 0)];
        let unit = composite(vec![nport("c"), nport("v")], Type::Number, vec![guard], edges.clone());
        let prepared = Prepared::of(&unit, &[guard], &edges, &r).unwrap().planned();
        assert!(prepared.plan.is_none());
    }

    /// Every genesis primitive is computed somewhere (D1): `compute` has an opcode for it, or it is
    /// a request the host answers, `map` or `fold`, or a source the host stands a constant in for
    /// (every capability it has is sourced). And every opcode is a genesis primitive's, its name
    /// finding it. A primitive added without its computation would otherwise read as one that is
    /// always absent.
    #[test]
    fn every_genesis_primitive_is_computed_somewhere_and_every_opcode_is_one() {
        let mut names = std::collections::BTreeSet::new();
        for (name, op) in Op::ALL {
            assert!(names.insert(*name), "{name} listed twice");
            assert_eq!(Op::of(name), Some(*op), "{name}");
            assert!(crate::GENESIS.contains(name), "{name} has an opcode but isn't a genesis primitive");
        }
        for name in crate::GENESIS {
            let unit = crate::interface_of(name).unwrap_or_else(|| panic!("{name}: no interface"));
            let sourced = crate::capabilities_of(name, &unit.params).is_some_and(|caps| caps.iter().all(|c| c.is_sourced()) && caps.iter().next().is_some());
            assert!(
                Op::of(name).is_some() || Request::of(name).is_some() || matches!(*name, "map" | "fold") || sourced,
                "{name} is computed nowhere: no opcode, no request, and not a source the host stands in for"
            );
        }
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
            fn asserted(&mut self, path: &[u32], _unit: &Unit, inputs: &[Option<&Value>]) -> Option<Vec<Option<Value>>> {
                self.2.push((path.to_vec(), inputs.iter().map(|o| o.cloned()).collect()));
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

    /// A sink's trigger (docs/L4s/triggers-L4.md §3): never read, dropped before it computes;
    /// wired and absent, it holds the sink back; open, the sink runs with the unit around it. A
    /// holon's own trigger left open reads as present, so its sinks run as a leaf's would.
    #[test]
    fn a_sink_waits_for_its_trigger_and_an_open_one_runs_with_its_unit() {
        let mut r = MapResolver::new();
        let write = Unit { params: vec![Param::frozen("key", "k")], ..crate::interface_of("store_write").unwrap() };
        assert_eq!(write.inputs.last().map(|p| p.name.as_str()), Some(crate::TRIGGER));
        let effect = |v: f64| Some(Value::Effect { sink: "store_write:k".into(), payload: Box::new(num(v)) });
        // The leaf itself: the trigger's value is never read.
        assert_eq!(eval(&write, &[Some(num(1.0)), Some(Value::Text("anything".into()))], &r).into_vec(), vec![effect(1.0)]);
        let hw = r.insert(write).unwrap();
        let ports = vec![nport("value"), Port { name: crate::TRIGGER.into(), ty: Type::Any }];
        let tied = composite(ports, Type::Effect, vec![hw], vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, BOUNDARY, 0)]);
        let open = composite(vec![nport("value")], Type::Effect, vec![hw], vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, BOUNDARY, 0)]);
        let run = |u: &Unit, inputs: &[Option<Value>], r: &MapResolver| {
            let lazy = eval(u, inputs, r).into_vec();
            assert_eq!(lazy, crate::eager::eval_eager(u, inputs, r), "eager agrees");
            lazy
        };
        assert_eq!(run(&tied, &[Some(num(2.0)), Some(Value::Boolean(true))], &r), vec![effect(2.0)]);
        assert_eq!(run(&tied, &[Some(num(2.0)), None], &r), vec![None], "wired, absent: held");
        assert_eq!(run(&open, &[Some(num(3.0))], &r), vec![effect(3.0)], "open: runs with its unit");
        // The tied one placed as a holon, its trigger left open: its sink runs.
        let holon = r.insert(tied).unwrap();
        let placed = composite(vec![nport("value")], Type::Effect, vec![holon], vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, BOUNDARY, 0)]);
        assert_eq!(run(&placed, &[Some(num(4.0))], &r), vec![effect(4.0)]);
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
    fn a_composite_nested_as_deep_as_a_host_runs_fits_half_a_threads_stack() {
        // The server refuses closures nested past 64 units (`MAX_CLOSURE_DEPTH`) because eval
        // recurses a level per nested composite; its threads have 2 MiB. 64 levels in 1 MiB,
        // unoptimized, leaves the rest for the host around it.
        let mut r = MapResolver::new();
        // `add`, `add2` around it, 61 wrappers, the top: 64 units deep.
        let inner = add2(&mut r);
        let mut below = r.insert(inner).unwrap();
        for _ in 0..61 {
            let wrap = composite(
                vec![nport("x"), nport("y")],
                Type::Number,
                vec![below],
                vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, BOUNDARY, 0)],
            );
            below = r.insert(wrap).unwrap();
        }
        let top = composite(
            vec![nport("x"), nport("y")],
            Type::Number,
            vec![below],
            vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, BOUNDARY, 0)],
        );
        let out = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(move || eval(&top, &[Some(num(10.0)), Some(num(5.0))], &r).into_vec())
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(out, vec![Some(num(15.0))]);
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
