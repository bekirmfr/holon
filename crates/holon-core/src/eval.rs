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
use crate::{lists, primitives, records, Edge, Hash, Param, Resolver, Unit, UnitBody, Value, BOUNDARY, VARIADIC};
use crate::hash::parse_hash;
use crate::value_hash::{nests_within, MAX_VALUE_DEPTH};
use smallvec::{smallvec, SmallVec};
use std::borrow::Cow;
use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::rc::Rc;
use std::sync::Arc;

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
    /// Whether [`produced`](Observer::produced) looks at the outputs it is handed: yes (the
    /// default) for anything that keeps or shows them. One that doesn't says so, and a choice
    /// (`guard`, `coalesce`) that passes on a value it can't move, one its composite was lent or
    /// one read elsewhere too, then lends it to its readers instead of keeping a copy as its own
    /// output; it is reported `produced` with no outputs. A list a function uses twice goes
    /// through a shared `guard`, so this is the copy of that list on every call.
    fn reads_produced(&self) -> bool {
        true
    }
}

/// Observes nothing: what plain [`eval`] uses.
pub struct NoObserver;
impl Observer for NoObserver {
    fn reads_produced(&self) -> bool {
        false
    }
}

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

/// A node's inputs when some may be given: each lent (`Cow::Borrowed`) as [`Refs`] lends it, or
/// given (`Cow::Owned`), moved out of where it was kept because nothing reads it there again, so
/// the node builds on it instead of copying it (a `fold`'s accumulator, `append`'s list; see
/// [`Prepared::fire_giving`]).
type Args<'v> = SmallVec<[Option<Cow<'v, Value>>; 4]>;

/// `inputs`, every one lent.
fn lent<'v>(inputs: &[Option<&'v Value>]) -> Args<'v> {
    inputs.iter().map(|v| v.map(Cow::Borrowed)).collect()
}

/// `args` as [`Refs`]: each read where it is, given or lent.
fn refs<'a>(args: &'a [Option<Cow<'_, Value>>]) -> Refs<'a> {
    args.iter().map(|v| v.as_deref()).collect()
}

/// Whether copying `v` costs more than giving it, rather than lending it to be copied, costs the
/// evaluator: a number or a boolean never does; a text past 32 bytes does; a list or a record does
/// past 8 entries, or with an entry that isn't a number, a boolean or a short text. Looking one
/// level down, at 8 entries at most, keeps the question cheap. Giving pays about 1,770
/// instructions a step (performance.md, "Small values are copied"), more than copying `{ v, dev,
/// sq }`; a list of 1,000 is given, so a fold that builds one stays linear.
fn heavy(v: &Value) -> bool {
    fn small(v: &Value) -> bool {
        match v {
            Value::Number(_) | Value::Boolean(_) => true,
            Value::Text(t) => t.len() <= 32,
            Value::Bytes(b) => b.len() <= 32,
            _ => false,
        }
    }
    match v {
        Value::Number(_) | Value::Boolean(_) => false,
        Value::Text(t) => t.len() > 32,
        Value::Bytes(b) => b.len() > 32,
        Value::List(xs) => xs.len() > 8 || !xs.iter().all(small),
        Value::Record(fields) => fields.len() > 8 || !fields.values().all(small),
        Value::Sum { .. } | Value::Effect { .. } => true,
    }
}

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
/// its name at every evaluation. Per run, unless the resolver keeps the children it resolves
/// ([`Resolved`]): another may hand a run its own unit for a hash (a design mode's mock).
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
    /// A higher-order leaf that tests each item with its function ([`eval_test`]).
    Test(Test),
    /// `sort_by`, ordering the items by its function ([`eval_sort_by`]).
    SortBy,
    /// `group_by`, putting the items in lists by the key its function answers ([`eval_group_by`]).
    GroupBy,
    /// `json_parse` with a reviver, `json_stringify` with a replacer ([`eval_json`]); without one,
    /// each is a [`Body::Leaf`].
    Json(Json),
    /// A request the host answers.
    Ask(Request),
    /// A leaf whose value the host supplies ([`crate::SOURCED`], [`Resolver::sourced`]).
    Sourced,
    Coalesce,
    /// `fail`: nothing, its message handed to the observer.
    Fail,
    /// Any other leaf, computed by `compute` (its [`Op`], none for a name that isn't one); a
    /// variadic one fires only with every input present.
    Leaf { variadic: bool, op: Option<Op> },
    Composite,
    Module,
}

/// The higher-order leaves that test each item with their function: `any`, `all`, `filter` and the
/// `find` family.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Test {
    Any,
    All,
    Filter,
    Find,
    FindIndex,
    FindLast,
    FindLastIndex,
}

impl Test {
    fn of(name: &str) -> Option<Test> {
        Some(match name {
            "any" => Test::Any,
            "all" => Test::All,
            "filter" => Test::Filter,
            "find" => Test::Find,
            "find_index" => Test::FindIndex,
            "find_last" => Test::FindLast,
            "find_last_index" => Test::FindLastIndex,
            _ => return None,
        })
    }

    /// Whether it answers with items of its list, which a list given to it moves rather than copies.
    fn keeps_items(self) -> bool {
        matches!(self, Test::Filter | Test::Find | Test::FindLast)
    }
}

/// The JSON leaves when their `fn` is set: `json_parse`'s reviver, `json_stringify`'s replacer.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Json {
    Parse,
    Stringify,
}

impl Json {
    fn of(name: &str, params: &[Param]) -> Option<Json> {
        let json = match name {
            "json_parse" => Json::Parse,
            "json_stringify" => Json::Stringify,
            _ => return None,
        };
        params.iter().any(|p| p.name() == "fn" && p.value().is_some_and(|f| !f.is_empty())).then_some(json)
    }
}

/// The requests a node asks the host, by the leaf that builds them.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Request {
    Http,
    Upload,
    FlowCall,
    FlowStart,
    Stream,
    Await,
    Approval,
    CollectionGet,
    CollectionList,
    FileRead,
    FileReadBytes,
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
            "flow_start" => Request::FlowStart,
            "http_stream" => Request::Stream,
            "await" => Request::Await,
            "approval" => Request::Approval,
            "collection_get" => Request::CollectionGet,
            "collection_list" => Request::CollectionList,
            "file_read" => Request::FileRead,
            "file_read_bytes" => Request::FileReadBytes,
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
            Request::FlowCall => primitives::flow_call_request(&owned(inputs), params, false),
            // A start of one: the host runs it until it finishes or first waits, and answers then.
            Request::FlowStart => primitives::flow_call_request(&owned(inputs), params, true),
            // One whose answer the host forwards to the route's caller as it arrives.
            Request::Stream => primitives::http_stream(&owned(inputs), params),
            // A durable run's waiting point and its token: also the host's to answer.
            Request::Await => primitives::await_request(params),
            Request::ResumeToken => primitives::resume_token_request(params),
            Request::Approval => primitives::approval_request(params),
            // A collection read: the host answers it from the collection as the run found it.
            Request::CollectionGet => primitives::collection_get_request(&owned(inputs), params),
            Request::CollectionList => primitives::collection_list_request(&owned(inputs), params),
            // A file read: the host answers it from the folder bound to the name, as the run found it.
            Request::FileRead => primitives::file_read_request(&owned(inputs), params),
            Request::FileReadBytes => primitives::file_read_bytes_request(&owned(inputs), params),
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
                    "sort_by" => Body::SortBy,
                    "group_by" => Body::GroupBy,
                    "coalesce" => Body::Coalesce,
                    "fail" => Body::Fail,
                    _ if crate::SOURCED.contains(&name) => Body::Sourced,
                    _ => match (Test::of(name), Request::of(name), Json::of(name, &unit.params)) {
                        (Some(test), _, _) => Body::Test(test),
                        (None, Some(request), _) => Body::Ask(request),
                        (None, None, Some(json)) => Body::Json(json),
                        (None, None, None) => Body::Leaf { variadic: VARIADIC.contains(&name), op: Op::of(name) },
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
        (Body::Map, _) => smallvec![eval_map(&mut lent(inputs), &unit.params, resolver, observer, path)],
        // `fold` is the one iteration primitive (the universal list donor). It threads an
        // accumulator, which `map` does not.
        (Body::Fold, _) => smallvec![eval_fold(&mut lent(inputs), &unit.params, resolver, observer, path)],
        (Body::Test(test), _) => smallvec![eval_test(test, &mut lent(inputs), &unit.params, resolver, observer, path)],
        (Body::SortBy, _) => smallvec![eval_sort_by(&mut lent(inputs), &unit.params, resolver, observer, path)],
        (Body::GroupBy, _) => smallvec![eval_group_by(&mut lent(inputs), &unit.params, resolver, observer, path)],
        (Body::Json(json), _) => smallvec![eval_json(json, &mut lent(inputs), &unit.params, resolver, observer, path)],
        (Body::Ask(request), _) => ask(request.build(inputs, &unit.params), observer, path),
        (Body::Sourced, _) => smallvec![Slot::of(crate::sourced_value(unit, resolver.sourced(unit)))],
        // `coalesce` is the one leaf whose meaning is about absence: the first input that
        // arrived. The generic arm below would give the same answer today (it gathers present
        // inputs in port order), but that is a property of its implementation, not its
        // contract, and the firing rule it serves says the opposite ("a wired input absent,
        // nothing fires"). This arm is the contract, stated once, where absence is visible.
        // (Inside a composite it is a choice, decided by `choice_next` without getting here.)
        (Body::Coalesce, _) => smallvec![Slot::of(inputs.iter().flatten().next().map(|v| (*v).clone()))],
        // `fail` gives nothing, and the observer hears why, at this node, as for a module that
        // can't run: the language's `throw new Error(message)`, its message for the run's log.
        (Body::Fail, _) => {
            if let Some(Some(message)) = inputs.first() {
                let reason = match message {
                    Value::Text(t) => t.clone(),
                    other => primitives::compute("to_text", &[other], &[]).map_or_else(String::new, |t| if let Value::Text(t) = t { t } else { String::new() }),
                };
                observer.failed(path, &reason);
            }
            smallvec![Slot::Absent]
        }
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
        (_, UnitBody::Composite { children, edges }) => eval_composite(unit, children, edges, &mut lent(inputs), resolver, observer, path),
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

/// [`eval_as`] on inputs some of which may be given (a `fold`'s accumulator, a value its last
/// reader takes): a leaf builds on what it is given ([`primitives::compute_given`]), a `map`,
/// `fold` or composite hands it on, and anything else reads it as it would read it lent. The
/// outputs are `eval_as`'s on the same values.
///
/// A `map`, `fold` or composite is handed `args` as they are, given or lent, rather than lent
/// again through `eval_as`: the recursion from one function into the next comes this way, and
/// goes through one frame fewer.
#[inline(never)]
fn eval_given<'r>(unit: &'r Unit, kind: &NodeKind, args: &mut Args<'_>, resolver: &'r impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slots {
    // A sink's trigger only has to be there, as `eval_as` reads it.
    let n = if kind.takes_trigger && args.len() == unit.inputs.len() { args.len() - 1 } else { args.len() };
    match (kind.body, &unit.body) {
        (Body::Map, _) => smallvec![eval_map(&mut args[..n], &unit.params, resolver, observer, path)],
        (Body::Fold, _) => smallvec![eval_fold(&mut args[..n], &unit.params, resolver, observer, path)],
        (Body::Test(test), _) => smallvec![eval_test(test, &mut args[..n], &unit.params, resolver, observer, path)],
        (Body::SortBy, _) => smallvec![eval_sort_by(&mut args[..n], &unit.params, resolver, observer, path)],
        (Body::GroupBy, _) => smallvec![eval_group_by(&mut args[..n], &unit.params, resolver, observer, path)],
        (Body::Json(json), _) => smallvec![eval_json(json, &mut args[..n], &unit.params, resolver, observer, path)],
        (_, UnitBody::Composite { children, edges }) => eval_composite(unit, children, edges, &mut args[..n], resolver, observer, path),
        (Body::Leaf { variadic, op: Some(op) }, UnitBody::Primitive { name }) if args.iter().any(|a| matches!(a, Some(Cow::Owned(v)) if heavy(v))) => {
            smallvec![leaf_given(op, name, variadic, &mut args[..n], &unit.params)]
        }
        // Nothing given worth building on (a `fold` of numbers), or a node that only reads: lent,
        // as always.
        _ => eval_as(unit, kind, &refs(args), resolver, observer, path),
    }
}

/// A leaf computed on `args`, building on what is given: what is given is the leaf's; what is
/// lent stays where it is, for the caller's next application (a `map`'s extras). Apart from
/// [`eval_given`], so the recursion through it doesn't carry this frame.
#[inline(never)]
fn leaf_given(op: Op, name: &str, variadic: bool, args: &mut [Option<Cow<'_, Value>>], params: &[Param]) -> Slot {
    if variadic && args.iter().any(Option::is_none) {
        return Slot::Absent;
    }
    let present = args
        .iter_mut()
        .filter_map(|arg| match arg {
            Some(Cow::Owned(_)) => arg.take(),
            Some(Cow::Borrowed(v)) => Some(Cow::Borrowed(*v)),
            None => None,
        })
        .collect();
    Slot::of(primitives::compute_given(op, name, present, params).ok())
}

/// What a leaf that produced nothing hands back of what it was given, by port ([`give_to_leaf`]).
type Back = SmallVec<[(usize, Value); 2]>;

/// A leaf, child of a composite, computed on `args` some of which are given ([`Prepared::fire_giving`]):
/// its outputs, and when it produces nothing, what it was given, by port, to be put back where it
/// was kept. Apart from `fire_giving`, so the recursion through it doesn't carry this frame.
#[inline(never)]
fn give_to_leaf(op: Op, name: &str, variadic: bool, args: Args<'_>, params: &[Param]) -> (Slots, Back) {
    let back = |args: Args| -> Back {
        args.into_iter()
            .enumerate()
            .filter_map(|(p, v)| match v {
                Some(Cow::Owned(v)) => Some((p, v)),
                _ => None,
            })
            .collect()
    };
    if variadic && args.iter().any(Option::is_none) {
        return (smallvec![Slot::Absent], back(args));
    }
    // Present inputs in port order, as a lent leaf computes on them; what comes back is what went
    // in, in order, each put back at its own port.
    let ports: SmallVec<[usize; 4]> = (0..args.len()).filter(|&p| args[p].is_some()).collect();
    let present = args.into_iter().flatten().collect();
    match primitives::compute_given(op, name, present, params) {
        Ok(v) => (smallvec![Slot::Value(v)], Back::new()),
        Err(returned) => {
            let mut at: Args = smallvec![None; ports.last().map_or(0, |&p| p + 1)];
            for (v, p) in returned.into_iter().zip(ports) {
                at[p] = Some(v);
            }
            (smallvec![Slot::Absent], back(at))
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
    let which = match name {
        "guard" => Which::Guard,
        "coalesce" => Which::Coalesce,
        _ => return None,
    };
    let at = |p: usize| seen.get(p).map_or(Some(In::Absent), |s| s.as_ref().map(In::of_slot));
    Some(match choose(which, at) {
        Chosen::Need(p) => Choice::Need(p),
        Chosen::Done(slot, _) => Choice::Done(slot.owned()),
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

/// [`Choice`], its output borrowed, with the port it was read from when it is an input's.
enum Chosen<'a> {
    Need(usize),
    Done(In<'a>, Option<usize>),
}

/// [`choice_next`]'s rule, over the inputs as `at` gives them (`None`: not yet evaluated).
fn choose<'a>(which: Which, at: impl Fn(usize) -> Option<In<'a>>) -> Chosen<'a> {
    match which {
        Which::Guard => match at(0) {
            None => Chosen::Need(0),
            Some(In::Pending) => Chosen::Done(In::Pending, None),
            Some(In::Value(Value::Boolean(true))) => match at(1) {
                None => Chosen::Need(1),
                Some(v) => Chosen::Done(v, Some(1)),
            },
            // false, absent, or not a boolean: absent, as `guard` is.
            Some(_) => Chosen::Done(In::Absent, None),
        },
        Which::Coalesce => match at(0) {
            None => Chosen::Need(0),
            Some(In::Absent) => match at(1) {
                None => Chosen::Need(1),
                Some(v) => Chosen::Done(v, Some(1)),
            },
            Some(first) => Chosen::Done(first, Some(0)),
        },
    }
}

fn eval_composite<'r>(
    unit: &'r Unit,
    children: &[Hash],
    edges: &[Edge],
    inputs: &mut [Option<Cow<'_, Value>>],
    resolver: &'r impl Resolver,
    observer: &mut impl Observer,
    path: &mut Vec<u32>,
) -> Slots {
    let n_outputs = unit.outputs.len();
    match Prepared::of(unit, children, edges, resolver) {
        Some(prepared) => prepared.run(n_outputs, inputs, resolver, observer, path),
        None => smallvec![Slot::Absent; n_outputs],
    }
}

/// How many shapes each thread keeps for units no resolver keeps one for ([`kept_shape`]).
const KEPT_SHAPES: usize = 64;

/// A shape kept by [`kept_shape`], with what it was made from.
#[derive(Debug)]
struct KeptShape {
    /// Where the unit it was made for lived.
    at: usize,
    children: usize,
    edges: Box<[Edge]>,
    shape: Shape,
}

thread_local! {
    /// The shapes [`kept_shape`] made last, by where their units live.
    static KEPT: RefCell<[Option<Rc<KeptShape>>; KEPT_SHAPES]> = const { RefCell::new([const { None }; KEPT_SHAPES]) };
}

/// The shape of `unit` (a composite of `children` children joined by `edges`), for a resolver that
/// keeps none: kept from the last time a unit at the same place had the same edges, else made and
/// kept. Working a shape out takes a dozen small allocations, more than running a small composite
/// does, and a composite is mostly evaluated again where it was before (a function called for
/// every request, one placed inside a `map`'s function). A shape is a function of the children's
/// count and the edges alone, and one is used only for a unit with exactly those, so whatever unit
/// lives at an address now, the shape found there is its own. A few dozen per thread, each place
/// keeping the last unit found there.
#[inline(never)]
fn kept_shape(unit: &Unit, children: usize, edges: &[Edge]) -> Rc<KeptShape> {
    let at = unit as *const Unit as usize;
    let place = (((at >> 4) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize % KEPT_SHAPES;
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        if let Some(found) = &kept[place]
            && found.at == at
            && found.children == children
            && *found.edges == *edges
        {
            return found.clone();
        }
        let made = Rc::new(KeptShape { at, children, edges: edges.into(), shape: Shape::of(children, edges) });
        kept[place] = Some(made.clone());
        made
    })
}

/// A composite's shape, as a run has it: the resolver's ([`Resolver::shape`]), or one kept for it.
enum ShapeOf<'r> {
    Resolver(&'r Shape),
    Kept(Rc<KeptShape>),
}

impl std::ops::Deref for ShapeOf<'_> {
    type Target = Shape;
    fn deref(&self) -> &Shape {
        match self {
            ShapeOf::Resolver(shape) => shape,
            ShapeOf::Kept(kept) => &kept.shape,
        }
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

/// A composite's children as a host's resolver resolves them, what each is, and the composite's
/// [`Shape`]: what preparing it for a run works out, kept beside the units ([`Shapes::kept`],
/// [`Resolver::resolved`]) so that one call of it pays its run and not its preparation
/// (js-library-races/leaves.md §8.1). The children are the very units the resolver holds, shared.
#[derive(Clone, Debug)]
pub struct Resolved {
    shape: Shape,
    units: Box<[Arc<Unit>]>,
    /// What each child is ([`NodeKind`]): a function of the child alone.
    kinds: Box<[NodeKind]>,
}

impl Resolved {
    fn of(shape: Shape, units: Box<[Arc<Unit>]>) -> Resolved {
        let kinds = units.iter().map(|u| NodeKind::of(u)).collect();
        Resolved { shape, units, kinds }
    }
}

/// What a host keeps of one composite: its shape, and its children when they all resolved.
#[derive(Clone, Debug)]
enum Kept {
    Shape(Shape),
    Resolved(Resolved),
}

/// The shapes of a set of units a host keeps, each found by the unit itself: by where it lives,
/// not by its hash, so finding one costs no hashing and a unit a resolver makes up for one run (a
/// design mode's mock) never meets a shape that isn't its own. Built with [`Shapes::kept`], it keeps
/// each composite's children resolved beside its shape ([`Resolved`]).
///
/// Sound as long as the units outlive the table and don't move: an address the table holds is
/// then the address of that very unit, and no other unit can live there. A host builds it over
/// units it holds behind a pointer (an `Arc`, a `Box`) and keeps the two together.
#[derive(Clone, Debug, Default)]
pub struct Shapes(HashMap<usize, Kept, BuildHasherDefault<AddressHasher>>);

impl Shapes {
    /// The shape of each composite among `units`.
    pub fn of<'u>(units: impl IntoIterator<Item = &'u Unit>) -> Shapes {
        let mut shapes = HashMap::default();
        for unit in units {
            if let Some(shape) = Shape::of_unit(unit) {
                shapes.insert(unit as *const Unit as usize, Kept::Shape(shape));
            }
        }
        Shapes(shapes)
    }

    /// [`Shapes::of`], with each composite's children resolved by `resolve` and kept beside its
    /// shape ([`Resolved`]): for a host whose resolver hands out, for each hash, the unit `resolve`
    /// gives and no other (the server's, holon's `units.rs`). A composite one of whose children
    /// doesn't resolve keeps its shape alone.
    pub fn kept<'u>(units: impl IntoIterator<Item = &'u Arc<Unit>>, resolve: impl Fn(&Hash) -> Option<&'u Arc<Unit>>) -> Shapes {
        let mut kept = HashMap::default();
        for unit in units {
            let UnitBody::Composite { children, edges } = &unit.body else { continue };
            let shape = Shape::of(children.len(), edges);
            let entry = match children.iter().map(|h| resolve(h).cloned()).collect::<Option<Box<[Arc<Unit>]>>>() {
                Some(units) => Kept::Resolved(Resolved::of(shape, units)),
                None => Kept::Shape(shape),
            };
            kept.insert(&**unit as *const Unit as usize, entry);
        }
        Shapes(kept)
    }

    fn find(&self, unit: &Unit) -> Option<&Kept> {
        if self.0.is_empty() {
            return None;
        }
        self.0.get(&(unit as *const Unit as usize))
    }

    /// `unit`'s shape, when it is one of the units the table was built over.
    pub fn get(&self, unit: &Unit) -> Option<&Shape> {
        match self.find(unit)? {
            Kept::Shape(shape) => Some(shape),
            Kept::Resolved(resolved) => Some(&resolved.shape),
        }
    }

    /// `unit`'s children, when it is one of the units the table was built over and they were kept
    /// ([`Shapes::kept`]).
    pub fn resolved(&self, unit: &Unit) -> Option<&Resolved> {
        match self.find(unit)? {
            Kept::Resolved(resolved) => Some(resolved),
            Kept::Shape(_) => None,
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// About what the table takes in memory: its map (a bucket and a half an entry), each shape's
    /// lists, and each kept child's pointer and kind. For a cache that counts what it keeps
    /// (holon's flow cache).
    pub fn footprint(&self) -> u64 {
        const ALLOC: u64 = 16;
        let bucket = (std::mem::size_of::<usize>() + std::mem::size_of::<Kept>()) as u64 + 1;
        self.0
            .values()
            .map(|kept| {
                let (shape, children) = match kept {
                    Kept::Shape(shape) => (shape, 0),
                    Kept::Resolved(r) => (&r.shape, (r.units.len() * 8 + r.kinds.len() * std::mem::size_of::<NodeKind>()) as u64 + 2 * ALLOC),
                };
                let i = &shape.incoming;
                bucket * 3 / 2
                    + (i.starts.capacity() * 4) as u64
                    + (i.ports.capacity() * std::mem::size_of::<Source>()) as u64
                    + (i.outputs.capacity() * std::mem::size_of::<Option<(u32, u32)>>()) as u64
                    + (shape.roots.capacity() * 8) as u64
                    + 4 * ALLOC
                    + children
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
/// nothing an application changes. Its children are taken from the resolver when it keeps them
/// ([`Resolved`]), and otherwise resolved for each run: a resolver may hand a run its own unit
/// under a hash (a design mode's mock).
struct Prepared<'r> {
    unit: &'r Unit,
    resolved: Vec<&'r Unit>,
    /// What each child is ([`NodeKind`]), decided once here.
    kinds: Vec<NodeKind>,
    shape: ShapeOf<'r>,
    /// The walk's scratch, kept for the next run of the same preparation (a `map`'s next item):
    /// emptied after each run, its room kept.
    scratch: RefCell<Vec<Option<Slots>>>,
    /// The walk worked out ahead ([`Plan`]), for one run many times over: none, walked.
    plan: Option<Plan>,
    /// What a preparation run many times keeps beside it ([`Many`]); none for one run once.
    many: Option<Box<Many<'r>>>,
}

/// One step of a [`Plan`].
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// Evaluate this child, every child feeding it evaluated already.
    Eval(u32),
    /// The same, for a child that builds on what it reads at some port ([`builds`]): it may be
    /// given a value, so the run looks for one, and only for it.
    Build(u32),
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

/// What a preparation run many times (a `map`'s or `fold`'s function, and the composites inside
/// it) keeps beside it, worked out once for all its runs:
///
/// - each composite child's own preparation, made the first time the child runs instead of at
///   every run (a function that calls another function prepared it once per item);
/// - each `const` child's value, its literal read once instead of at every run;
/// - for each input port, whether the value read there may be **given** to its node, moved out of
///   where it is kept instead of lent to be copied. A node is given a value when it builds on it
///   (`append` grows a list, a `fold` its accumulator, a composite hands it on; [`builds`]) and
///   every other reader of the value is done with it: already evaluated, never to be evaluated in
///   this run, or unable to read it once the node has produced a value ([`Gives`]). So a `fold`
///   that grows its accumulator grows it in place, O(n) where copying it each step was O(n²).
struct Many<'r> {
    nested: Box<[OnceCell<Option<Box<Prepared<'r>>>>]>,
    /// What each `const` child computes (`compute` of its params alone); none for anything else.
    consts: Box<[Option<Slot>]>,
    /// Whether some port of each child builds on what it reads ([`builds`]).
    builds: Vec<bool>,
    /// Who reads what, worked out the first time a value worth giving reaches a port that builds on
    /// it: a function over numbers never needs it.
    gives: OnceCell<Gives>,
}

/// What can't be read in a run where a leaf produces a value ([`Prepared::excluded_by`]): the
/// reads (child, port) that won't happen, and, by child, whether it won't be evaluated.
type Excluded = (Vec<(u32, u32)>, Vec<bool>);

/// Which ports may be given their value, and what each waits on ([`Many`]).
struct Gives {
    /// Each child's ports in `gives`, from `starts[child]` to `starts[child + 1]`.
    starts: Vec<u32>,
    gives: Vec<Give>,
    /// The readers a [`Give`] waits on, each a child.
    waits: Vec<u32>,
    /// The children reading each child's outputs, `consumers[consumer_starts[c]..consumer_starts[c + 1]]`.
    consumer_starts: Vec<u32>,
    consumers: Vec<u32>,
    /// The walk's roots ([`Shape`]): evaluated whatever reads them.
    root: Vec<bool>,
}

/// Whether a node builds its output on what it reads at port `p`, so that giving it the value
/// saves a copy: a choice's value, a leaf that keeps its input ([`primitives::keeps`]), what a
/// composite is handed, a `map`'s list, a `fold`'s accumulator and list, and the list of a
/// higher-order leaf that answers with its items (`filter`, `find`, `find_last`, `sort_by`,
/// `group_by`).
fn builds(kind: &NodeKind, p: usize) -> bool {
    match (kind.choice, kind.body) {
        (Some(Which::Guard), _) => p == 1,
        (Some(Which::Coalesce), _) => p < 2,
        (None, Body::Leaf { op: Some(op), .. }) => !kind.takes_trigger && primitives::keeps(op, p),
        (None, Body::Composite) => true,
        (None, Body::Map) => p == 0,
        (None, Body::Fold) => p < 2,
        (None, Body::Test(test)) => p == 0 && test.keeps_items(),
        (None, Body::SortBy | Body::GroupBy) => p == 0,
        _ => false,
    }
}

/// Whether the value one port reads may be given to its node, and when.
#[derive(Clone, Copy, Debug, Default)]
struct Give {
    /// The node builds on the value when it is given (`primitives::keeps`, a choice's value, a
    /// composite's, `map`'s or `fold`'s input), and no output of the composite reads it.
    on: bool,
    /// The other readers that must be done with it first: `Many::waits[from..to]`.
    from: u32,
    to: u32,
    /// Some readers are passed over because they can't read it once this node, a leaf, has
    /// produced a value (a `coalesce` whose first input is that value): the leaf is given it
    /// before that is known, and it is put back if the leaf produces nothing.
    speculative: bool,
}

impl<'r> Many<'r> {
    /// Child `ci`'s own preparation, run many times as its parent is: made the first time.
    fn nested(&self, ci: usize, child: &'r Unit, resolver: &'r impl Resolver) -> Option<&Prepared<'r>> {
        #[inline(never)]
        fn prepare<'r>(child: &'r Unit, resolver: &'r impl Resolver) -> Option<Box<Prepared<'r>>> {
            match &child.body {
                UnitBody::Composite { children, edges } => Prepared::of(child, children, edges, resolver).map(|p| Box::new(p.planned())),
                _ => None,
            }
        }
        self.nested[ci].get_or_init(|| prepare(child, resolver)).as_deref()
    }
}

impl Gives {
    fn consumers(&self, c: usize) -> &[u32] {
        &self.consumers[self.consumer_starts[c] as usize..self.consumer_starts[c + 1] as usize]
    }

    /// Port `p` of child `ci`'s entry. Every port the walk reads has one ([`Prepared::gives_of`]);
    /// one that somehow hadn't is given nothing, rather than another port's entry read.
    fn give(&self, ci: usize, p: usize) -> Option<Give> {
        let (from, to) = (*self.starts.get(ci)? as usize, *self.starts.get(ci + 1)? as usize);
        (from + p < to).then(|| self.gives[from + p])
    }
}

/// Whether the walk will never evaluate child `c`: it isn't evaluated, isn't a root, and
/// everything reading it is evaluated (so won't ask for it) or never evaluated itself. Only
/// something still to run pulls a child in, so nothing will. Each child is looked at once a
/// question (`seen`: 1 unneeded, 2 needed), so the work is the composite's size whatever its
/// shape; past 64 readers up it is taken as needed, which only keeps a value where it is.
fn unneeded(gives: &Gives, c: usize, scratch: &[Option<Slots>], seen: &mut SmallVec<[u8; 32]>, depth: usize) -> bool {
    match seen.get(c) {
        Some(1) => return true,
        Some(2) => return false,
        _ => {}
    }
    let found = scratch[c].is_none() && !gives.root[c] && depth < 64 && gives.consumers(c).iter().all(|&r| scratch[r as usize].is_some() || unneeded(gives, r as usize, scratch, seen, depth + 1));
    if seen.len() < scratch.len() {
        seen.resize(scratch.len(), 0);
    }
    seen[c] = if found { 1 } else { 2 };
    found
}

impl<'r> Prepared<'r> {
    /// None when a child doesn't resolve or the edges hold a cycle: then nothing is evaluable,
    /// and every declared output slot is absent.
    fn of(unit: &'r Unit, children: &[Hash], edges: &[Edge], resolver: &'r impl Resolver) -> Option<Prepared<'r>> {
        // Kept by the resolver, resolved once: the same children, kinds and shape as below.
        if let Some(kept) = resolver.resolved(unit) {
            if kept.shape.cyclic {
                return None;
            }
            let resolved = kept.units.iter().map(|u| &**u).collect();
            let shape = ShapeOf::Resolver(&kept.shape);
            return Some(Prepared { unit, resolved, kinds: kept.kinds.to_vec(), shape, scratch: RefCell::new(Vec::new()), plan: None, many: None });
        }
        // Resolve children (by hash). Any unresolved ⇒ nothing evaluable — every declared output
        // slot is absent (the vec's *length* is still the declared contract; only the values are
        // absent, mirroring how `inputs` already has a fixed length with individually-absent slots).
        let mut resolved: Vec<&Unit> = Vec::with_capacity(children.len());
        for h in children {
            resolved.push(resolver.resolve(h)?);
        }
        let shape = match resolver.shape(unit) {
            Some(kept) => ShapeOf::Resolver(kept),
            None => ShapeOf::Kept(kept_shape(unit, children.len(), edges)),
        };
        if shape.cyclic {
            return None;
        }
        let kinds = resolved.iter().map(|u| NodeKind::of(u)).collect();
        Some(Prepared { unit, resolved, kinds, shape, scratch: RefCell::new(Vec::new()), plan: None, many: None })
    }

    /// Evaluate it on `inputs`: lent, or given when the caller has no further use for one (a
    /// `fold`'s accumulator), which the run may then build on.
    fn run(&self, n_outputs: usize, inputs: &mut [Option<Cow<'_, Value>>], resolver: &'r impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slots {
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

    /// It, to be run many times (a `map`'s or `fold`'s function): with its walk worked out ahead
    /// ([`Plan`]) when it has no choice, and what [`Many`] keeps.
    #[inline(never)]
    fn planned(mut self) -> Prepared<'r> {
        if (0..self.kinds.len()).all(|ci| self.kinds[ci].choice.is_none() || self.passes(ci)) {
            self.plan = Some(self.plan_of());
        }
        let n = self.resolved.len();
        let builds = (0..n).map(|c| (0..self.resolved[c].inputs.len()).any(|p| builds(&self.kinds[c], p))).collect();
        let consts = (0..n)
            .map(|c| match (self.kinds[c].body, &self.resolved[c].body) {
                (Body::Leaf { op: Some(Op::Const), .. }, UnitBody::Primitive { name }) if self.resolved[c].inputs.is_empty() => {
                    Some(Slot::of(primitives::compute_op(Op::Const, name, &[], &self.resolved[c].params)))
                }
                _ => None,
            })
            .collect();
        self.many = Some(Box::new(Many { nested: (0..n).map(|_| OnceCell::new()).collect(), consts, builds, gives: OnceCell::new() }));
        self
    }

    /// What [`Many`] keeps for which ports may be given their values, made the first time it is asked.
    fn gives<'m>(&self, many: &'m Many<'r>) -> &'m Gives {
        many.gives.get_or_init(|| self.gives_of())
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
            order.push(if (0..k).any(|p| builds(&self.kinds[ci], p)) { Step::Build(ci as u32) } else { Step::Eval(ci as u32) });
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

    /// Who reads each value a run keeps, and from that which ports may be given theirs ([`Gives`]).
    #[inline(never)]
    fn gives_of(&self) -> Gives {
        let incoming = &self.shape.incoming;
        let n = self.resolved.len();
        let n_in = self.unit.inputs.len();
        // The ports the walk reads: those a child declares, and a choice's two whatever it
        // declares (`choose` reads a `guard`'s value and a `coalesce`'s fallback at port 1), so
        // that a reader there is counted and its port has a place in `starts`.
        let ports = |c: usize| self.resolved[c].inputs.len().max(if self.kinds[c].choice.is_some() { 2 } else { 0 });
        let outs = |c: usize| self.resolved[c].outputs.len();
        // Every value a run keeps, numbered: the composite's inputs, then each child's outputs.
        let mut out_starts = Vec::with_capacity(n + 1);
        let mut values = n_in;
        for c in 0..n {
            out_starts.push(values);
            values += outs(c);
        }
        out_starts.push(values);
        let value_of = |s: Source| match s {
            Source::Input(fp) if (fp as usize) < n_in => Some(fp as usize),
            Source::Child(fc, fp) if (fp as usize) < outs(fc as usize) => Some(out_starts[fc as usize] + fp as usize),
            _ => None,
        };
        // Who reads each value: the ports the walk reads it at, and whether an output does.
        let mut reader_starts = vec![0u32; values + 1];
        for ci in 0..n {
            for p in 0..ports(ci) {
                if let Some(v) = value_of(incoming.source(ci, p)) {
                    reader_starts[v + 1] += 1;
                }
            }
        }
        for v in 0..values {
            reader_starts[v + 1] += reader_starts[v];
        }
        let mut readers = vec![(0u32, 0u32); reader_starts[values] as usize];
        let mut fill = reader_starts.clone();
        for ci in 0..n {
            for p in 0..ports(ci) {
                if let Some(v) = value_of(incoming.source(ci, p)) {
                    readers[fill[v] as usize] = (ci as u32, p as u32);
                    fill[v] += 1;
                }
            }
        }
        let readers_of = |v: usize| &readers[reader_starts[v] as usize..reader_starts[v + 1] as usize];
        let mut output = vec![false; values];
        for op in 0..self.unit.outputs.len() as u32 {
            match incoming.get(BOUNDARY, op) {
                Some((BOUNDARY, fp)) if (fp as usize) < n_in => output[fp as usize] = true,
                Some((fc, fp)) if (fc as usize) < n => {
                    if let Some(v) = value_of(Source::Child(fc, fp)) {
                        output[v] = true;
                    }
                }
                _ => {}
            }
        }
        let mut root = vec![false; n];
        for &c in &self.shape.roots {
            root[c] = true;
        }
        let mut consumer_starts = Vec::with_capacity(n + 1);
        let mut consumers = Vec::new();
        for c in 0..n {
            consumer_starts.push(consumers.len() as u32);
            let first = consumers.len();
            for v in out_starts[c]..out_starts[c + 1] {
                for &(r, _) in readers_of(v) {
                    if !consumers[first..].contains(&r) {
                        consumers.push(r);
                    }
                }
            }
        }
        consumer_starts.push(consumers.len() as u32);

        let mut starts = Vec::with_capacity(n + 1);
        starts.push(0u32);
        for c in 0..n {
            starts.push(starts[c] + ports(c) as u32);
        }
        let mut gives = vec![Give::default(); starts[n] as usize];
        let mut waits = Vec::new();
        let coalesces = self.kinds.iter().any(|k| k.choice == Some(Which::Coalesce));
        for ci in 0..n {
            let kind = &self.kinds[ci];
            let leaf = matches!((kind.choice, kind.body), (None, Body::Leaf { op: Some(_), .. })) && !kind.takes_trigger;
            // The leaves whose having produced a value means some readers won't read: worked out
            // once for the node, when it might matter.
            let mut excluded: Option<Excluded> = None;
            for p in 0..ports(ci) {
                let Some(v) = value_of(incoming.source(ci, p)).filter(|&v| builds(kind, p) && !output[v]) else { continue };
                let mut others: SmallVec<[(u32, u32); 4]> = readers_of(v).iter().copied().collect();
                if let Some(at) = others.iter().position(|&r| r == (ci as u32, p as u32)) {
                    others.remove(at);
                }
                // The node reads it at another port too: it can't be given one and lent the other.
                if others.iter().any(|&(c, _)| c as usize == ci) {
                    continue;
                }
                let mut speculative = false;
                let from = waits.len() as u32;
                for (c, q) in others {
                    if leaf && coalesces {
                        let (reads, nodes) = excluded.get_or_insert_with(|| self.excluded_by(ci, &out_starts, &readers_of, &output, &root));
                        if reads.contains(&(c, q)) || nodes[c as usize] {
                            speculative = true;
                            continue;
                        }
                    }
                    waits.push(c);
                }
                gives[(starts[ci] as usize) + p] = Give { on: true, from, to: waits.len() as u32, speculative };
            }
        }
        Gives { starts, gives, waits, consumer_starts, consumers, root }
    }

    /// What can't be read in a run where leaf `ci` produces a value: the reads (child, port) that
    /// won't happen, and the children that won't be evaluated. A `coalesce` reads its second input
    /// only when its first is absent, so if its first input is `ci`'s value, carried up through
    /// `guard`s that pass a value only when their condition is `true` (each link read by the next
    /// alone, so `ci` runs only when every condition on the way is), that read won't happen; and a
    /// child that isn't a root, read only where reading won't happen, isn't evaluated.
    fn excluded_by<'a>(&self, ci: usize, out_starts: &[usize], readers_of: &impl Fn(usize) -> &'a [(u32, u32)], output: &[bool], root: &[bool]) -> Excluded {
        let n = self.resolved.len();
        let mut reads = Vec::new();
        let mut chain = vec![false; n];
        let mut at = ci;
        loop {
            chain[at] = true;
            if self.resolved[at].outputs.len() != 1 || root[at] || output[out_starts[at]] {
                break;
            }
            let &[(g, q)] = readers_of(out_starts[at]) else { break };
            match self.kinds[g as usize].choice {
                Some(Which::Guard) if q == 1 => at = g as usize,
                Some(Which::Coalesce) if q == 0 => {
                    reads.push((g, 1));
                    at = g as usize;
                }
                _ => break,
            }
        }
        let mut nodes = vec![false; n];
        if reads.is_empty() {
            return (reads, nodes);
        }
        loop {
            let mut more = false;
            for c in 0..n {
                if nodes[c] || chain[c] || root[c] {
                    continue;
                }
                let unread = (out_starts[c]..out_starts[c + 1])
                    .all(|v| !output[v] && readers_of(v).iter().all(|&(r, p)| nodes[r as usize] || reads.contains(&(r, p))));
                if unread {
                    nodes[c] = true;
                    more = true;
                }
            }
            if !more {
                return (reads, nodes);
            }
        }
    }

    /// The value at port `p` of child `ci`, borrowed where it is kept.
    fn read<'s>(&self, ci: usize, p: usize, scratch: &'s [Option<Slots>], inputs: &'s [Option<Cow<'_, Value>>]) -> In<'s> {
        match self.shape.incoming.source(ci, p) {
            Source::Input(fp) => In::of_value(inputs.get(fp as usize).and_then(|v| v.as_deref())),
            Source::Child(fc, fp) => self.output(fc as usize, fp as usize, scratch, inputs).unwrap_or(In::Absent),
            Source::Unwired | Source::Missing(..) => In::Absent,
        }
    }

    /// Child `fc`'s output `fp`, borrowed where it is kept; `None` while `fc` isn't evaluated. A
    /// choice that lends what it decided on ([`lends`](Prepared::lends)) reads as that, where it
    /// is kept, at its one output; at a port it doesn't have, absent, as it reads when it keeps a
    /// copy (a choice unit may declare more outputs than it produces).
    #[inline]
    fn output<'s>(&self, fc: usize, fp: usize, scratch: &'s [Option<Slots>], inputs: &'s [Option<Cow<'_, Value>>]) -> Option<In<'s>> {
        let outs = scratch[fc].as_ref()?;
        Some(match outs.get(fp) {
            Some(slot) => In::of_slot(slot),
            None if fp == 0 => self.read_through(fc, scratch, inputs),
            None => In::Absent,
        })
    }

    /// What child `c`, evaluated, reads as at an output it doesn't keep: what it lends, if it is
    /// a choice that lends; else absent. Apart from [`output`](Prepared::output), which every
    /// read goes through, so that stays small.
    #[cold]
    #[inline(never)]
    fn read_through<'s>(&self, c: usize, scratch: &'s [Option<Slots>], inputs: &'s [Option<Cow<'_, Value>>]) -> In<'s> {
        if self.lends(c, scratch) {
            self.read(c, self.lent_port(c, scratch, inputs), scratch, inputs)
        } else {
            In::Absent
        }
    }

    /// Whether child `c` is a choice that lends what it decided on: kept as no outputs (a choice
    /// has one), its readers read through it ([`chosen`](Prepared::chosen)).
    fn lends(&self, c: usize, scratch: &[Option<Slots>]) -> bool {
        self.kinds[c].choice.is_some() && scratch[c].as_ref().is_some_and(|outs| outs.is_empty())
    }

    /// The port a choice that lends decided on, decided again: a `guard`'s value, a `coalesce`'s
    /// first input if that is a value, else its second. What it reads doesn't change after the
    /// choice (a value lent is never moved), so the choice comes out as it did.
    fn lent_port(&self, c: usize, scratch: &[Option<Slots>], inputs: &[Option<Cow<'_, Value>>]) -> usize {
        match self.kinds[c].choice {
            Some(Which::Coalesce) if !matches!(self.read(c, 0, scratch, inputs), In::Value(_)) => 1,
            Some(Which::Coalesce) => 0,
            _ => 1,
        }
    }

    /// Whether the value at port `p` of child `ci` may be given to it now, and if so whether only
    /// for as long as it produces a value (`Some(speculative)`): its port may be given one
    /// ([`Give`]), the value is worth moving and is the run's to move, and every other reader is
    /// done with it. Reading `walked`, a reader not yet evaluated whose readers are all done is
    /// done too: the walk only evaluates what something still to run needs. A choice that lends
    /// the value is never done with it: its readers read it where it is.
    fn may_give(&self, ci: usize, p: usize, scratch: &[Option<Slots>], inputs: &[Option<Cow<'_, Value>>], walked: bool) -> Option<bool> {
        let many = self.many.as_deref()?;
        if !(builds(&self.kinds[ci], p) && self.movable(ci, p, scratch, inputs)) {
            return None;
        }
        let gives = self.gives(many);
        let give = gives.give(ci, p)?;
        let mut seen = SmallVec::new();
        let done = |&r: &u32| (scratch[r as usize].is_some() && !self.lends(r as usize, scratch)) || walked && unneeded(gives, r as usize, scratch, &mut seen, 0);
        (give.on && gives.waits[give.from as usize..give.to as usize].iter().all(done)).then_some(give.speculative)
    }

    /// The value at port `p` of child `ci`, moved out of where it is kept (which then reads as
    /// absent: [`may_give`](Prepared::may_give) has found nothing will read it there).
    fn take(&self, ci: usize, p: usize, scratch: &mut [Option<Slots>], inputs: &mut [Option<Cow<'_, Value>>]) -> Option<Value> {
        match self.shape.incoming.source(ci, p) {
            Source::Input(fp) => match inputs.get_mut(fp as usize) {
                Some(slot @ Some(Cow::Owned(_))) => slot.take().map(Cow::into_owned),
                _ => None,
            },
            Source::Child(fc, fp) => scratch[fc as usize].as_mut().and_then(|o| o.get_mut(fp as usize)).and_then(|slot| std::mem::replace(slot, Slot::Absent).into_value()),
            Source::Unwired | Source::Missing(..) => None,
        }
    }

    /// Put `value`, taken for port `p` of child `ci` by [`take`](Prepared::take), back.
    fn put_back(&self, ci: usize, p: usize, value: Value, scratch: &mut [Option<Slots>], inputs: &mut [Option<Cow<'_, Value>>]) {
        match self.shape.incoming.source(ci, p) {
            Source::Input(fp) => inputs[fp as usize] = Some(Cow::Owned(value)),
            Source::Child(fc, fp) => {
                if let Some(slot) = scratch[fc as usize].as_mut().and_then(|o| o.get_mut(fp as usize)) {
                    *slot = Slot::Value(value);
                }
            }
            Source::Unwired | Source::Missing(..) => {}
        }
    }

    /// A choice's outputs, once it decided on the value at port `p`: the value given to it when
    /// nothing else will read it; else, when `lend` (the observer doesn't look at what it
    /// produced) and a copy would cost, none, its readers reading the value where it is
    /// ([`output`](Prepared::output)); else a copy.
    #[inline(never)]
    fn chosen(&self, ci: usize, p: usize, scratch: &mut [Option<Slots>], inputs: &mut [Option<Cow<'_, Value>>], walked: bool, lend: bool) -> Slots {
        if self.may_give(ci, p, scratch, inputs, walked).is_some() {
            return smallvec![Slot::of(self.take(ci, p, scratch, inputs))];
        }
        match self.read(ci, p, scratch, inputs) {
            In::Value(v) if lend && heavy(v) => Slots::new(),
            read => smallvec![read.owned()],
        }
    }

    /// Whether child `ci` goes through [`fire_giving`](Prepared::fire_giving): it builds on what it
    /// reads at some port, and a value worth giving is there, the run's to move. A value lent to
    /// the run (a `map`'s item) can't be given, so its node fires as any other.
    fn giving(&self, ci: usize, gathered: &[Option<&Value>], scratch: &[Option<Slots>], inputs: &[Option<Cow<'_, Value>>]) -> bool {
        self.many.as_ref().is_some_and(|many| many.builds[ci]) && (0..gathered.len()).any(|p| builds(&self.kinds[ci], p) && self.movable(ci, p, scratch, inputs))
    }

    /// Whether the value at port `p` of child `ci` is worth moving and the run's to move: an input
    /// the run owns, or a child's output.
    fn movable(&self, ci: usize, p: usize, scratch: &[Option<Slots>], inputs: &[Option<Cow<'_, Value>>]) -> bool {
        match self.shape.incoming.source(ci, p) {
            Source::Input(fp) => matches!(inputs.get(fp as usize), Some(Some(Cow::Owned(v))) if heavy(v)),
            Source::Child(fc, fp) => matches!(scratch[fc as usize].as_ref().and_then(|o| o.get(fp as usize)), Some(Slot::Value(v)) if heavy(v)),
            Source::Unwired | Source::Missing(..) => false,
        }
    }

    /// [`run_in`](Prepared::run_in), down its [`Plan`].
    #[allow(clippy::too_many_arguments)]
    fn run_planned(&self, plan: &Plan, scratch: &mut Vec<Option<Slots>>, n_outputs: usize, inputs: &mut [Option<Cow<'_, Value>>], resolver: &'r impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slots {
        scratch.resize(self.resolved.len(), None);
        let open = crate::open_trigger();
        for &step in &plan.order {
            let (ci, builder) = match step {
                Step::Eval(ci) => (ci as usize, false),
                Step::Build(ci) => (ci as usize, true),
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
                let ready = match self.shape.incoming.source(ci, 1) {
                    Source::Child(fc, _) => scratch[fc as usize].is_some(),
                    Source::Input(_) | Source::Unwired | Source::Missing(..) => true,
                };
                if cond != Some(&Slot::Value(Value::Boolean(true))) || !ready {
                    return self.run_in(scratch, n_outputs, inputs, resolver, observer, path);
                }
                let outs = self.chosen(ci, 1, scratch, inputs, false, !observer.reads_produced());
                scratch[ci] = Some(self.decided(ci, outs, observer, path));
                continue;
            }
            let fired = {
                let mut gathered: Refs = SmallVec::new();
                let mut pending = false;
                let mut ahead = false;
                for source in &plan.sources[plan.starts[ci] as usize..plan.starts[ci + 1] as usize] {
                    let slot = match *source {
                        Source::Input(fp) => In::of_value(inputs.get(fp as usize).and_then(|v| v.as_deref())),
                        Source::Child(fc, fp) => match self.output(fc as usize, fp as usize, scratch, inputs) {
                            Some(slot) => slot,
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
                if ahead {
                    Fired::Ahead
                } else if builder && !pending && self.giving(ci, &gathered, scratch, inputs) {
                    Fired::Giving
                } else {
                    Fired::Done(self.fire(ci, &mut gathered, pending, &open, resolver, observer, path))
                }
            };
            let outs = match fired {
                Fired::Done(outs) => outs,
                Fired::Giving => self.fire_giving(ci, scratch, inputs, false, resolver, observer, path),
                Fired::Ahead => return self.run_in(scratch, n_outputs, inputs, resolver, observer, path),
            };
            scratch[ci] = Some(outs);
        }
        self.outputs(scratch, n_outputs, inputs)
    }

    /// Evaluate child `ci` on its inputs (`gathered`, every one evaluated; `pending` when one is
    /// still waiting on an answer), telling the observer: what the walk and the plan do with a
    /// child once its inputs are in.
    #[allow(clippy::too_many_arguments)]
    fn fire<'v>(&self, ci: usize, gathered: &mut Refs<'v>, pending: bool, open: &'v Value, resolver: &'r impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slots {
        let (child, kind) = (self.resolved[ci], &self.kinds[ci]);
        if pending {
            // Waiting on an answer: not run in this round.
            return smallvec![Slot::Pending; child.outputs.len()];
        }
        self.told(ci, observer, path, |observer, path| match self.ahead(ci, gathered, open, observer, path) {
            Some(outs) => outs,
            None => match (kind.body, &self.many) {
                // A composite child of a preparation run many times runs its own, kept.
                (Body::Composite, Some(many)) => match many.nested(ci, child, resolver) {
                    Some(nested) => nested.run(child.outputs.len(), &mut lent(gathered), resolver, observer, path),
                    None => smallvec![Slot::Absent; child.outputs.len()],
                },
                // A `const`: what it computed once.
                (Body::Leaf { .. }, Some(many)) if many.consts[ci].is_some() => many.consts[ci].iter().cloned().collect(),
                _ => eval_as(child, kind, gathered, resolver, observer, path),
            },
        })
    }

    /// `eval`, told to the observer as child `ci`'s evaluation: entered, produced and left, at
    /// its path. Inlined, so it adds no frame to the recursion through `fire`.
    #[inline(always)]
    fn told<O: Observer>(&self, ci: usize, observer: &mut O, path: &mut Vec<u32>, eval: impl FnOnce(&mut O, &mut Vec<u32>) -> Slots) -> Slots {
        path.push(ci as u32);
        observer.enter(path);
        let outs = eval(observer, path);
        observer.produced(path, &outs);
        observer.leave(path);
        path.pop();
        outs
    }

    /// What child `ci` produces without being computed, if anything: held, or what the observer
    /// stands in for it ([`Observer::asserted`]), on its inputs (`gathered`, which gets the open
    /// triggers it reads). Asked once the child is entered.
    fn ahead<'v>(&self, ci: usize, gathered: &mut Refs<'v>, open: &'v Value, observer: &mut impl Observer, path: &[u32]) -> Option<Slots> {
        let (child, kind, incoming) = (self.resolved[ci], &self.kinds[ci], &self.shape.incoming);
        // A leaf with optional ports (an `http_request`'s headers and body) reads an *unwired*
        // one as "none"; a *wired* one whose value is absent means what absence means everywhere
        // else: the node doesn't fire. Otherwise a request downstream of a check that failed (a
        // webhook whose signature didn't verify) still went out, bodiless.
        open_triggers(child, gathered, || open, |p| incoming.wired(ci as u32, p as u32));
        // A sink is held the same way: its trigger, wired, hasn't arrived.
        if kind.held_when_absent && (0..child.inputs.len()).any(|p| gathered[p].is_none() && incoming.wired(ci as u32, p as u32)) {
            return Some(smallvec![Slot::Absent; child.outputs.len()]);
        }
        observer.asserted(path, child, gathered).filter(|a| a.len() == child.outputs.len()).map(|a| a.into_iter().map(Slot::of).collect())
    }

    /// [`fire`](Prepared::fire), for a child of a preparation run many times that may be given
    /// some of its inputs ([`Many`]), none of them pending. It is told about exactly as `fire`
    /// tells it, and asserted, held and computed on the same values; then each input nothing
    /// else will read is moved out of where it is kept and given to it, the others lent. A leaf
    /// that produces nothing hands what it was given back ([`give_to_leaf`]), so a reader passed
    /// over because the leaf would produce a value (a `coalesce`'s fallback) still finds it.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn fire_giving(&self, ci: usize, scratch: &mut [Option<Slots>], inputs: &mut [Option<Cow<'_, Value>>], walked: bool, resolver: &'r impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slots {
        let (child, kind, incoming) = (self.resolved[ci], &self.kinds[ci], &self.shape.incoming);
        let Some(many) = self.many.as_deref() else { return smallvec![Slot::Absent; child.outputs.len()] };
        let k = child.inputs.len();
        let open = crate::open_trigger();
        self.told(ci, observer, path, |observer, path| {
            let ahead = {
                let mut gathered: Refs = (0..k).map(|p| self.read(ci, p, scratch, inputs).value()).collect();
                self.ahead(ci, &mut gathered, &open, observer, path)
            };
            if let Some(outs) = ahead {
                return outs;
            }
            let given: SmallVec<[Option<Value>; 4]> = (0..k).map(|p| self.may_give(ci, p, scratch, inputs, walked).and_then(|_| self.take(ci, p, scratch, inputs))).collect();
            let (outs, back) = {
                let mut args: Args = given.into_iter().enumerate().map(|(p, v)| v.map(Cow::Owned).or_else(|| self.read(ci, p, scratch, inputs).value().map(Cow::Borrowed))).collect();
                open_triggers(child, &mut args, || Cow::Borrowed(&open), |p| incoming.wired(ci as u32, p as u32));
                match (kind.body, &child.body) {
                    (Body::Leaf { variadic, op: Some(op) }, UnitBody::Primitive { name }) if !kind.takes_trigger => give_to_leaf(op, name, variadic, args, &child.params),
                    (Body::Composite, _) => match many.nested(ci, child, resolver) {
                        Some(nested) => (nested.run(child.outputs.len(), &mut args, resolver, observer, path), Back::new()),
                        None => (smallvec![Slot::Absent; child.outputs.len()], Back::new()),
                    },
                    _ => (eval_given(child, kind, &mut args, resolver, observer, path), Back::new()),
                }
            };
            for (p, v) in back {
                self.put_back(ci, p, v, scratch, inputs);
            }
            outs
        })
    }

    /// [`run`](Prepared::run), in `scratch` (empty).
    fn run_in(&self, scratch: &mut Vec<Option<Slots>>, n_outputs: usize, inputs: &mut [Option<Cow<'_, Value>>], resolver: &'r impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slots {
        let (resolved, incoming) = (&self.resolved, &self.shape.incoming);
        let n = resolved.len();
        let mut stack: SmallVec<[usize; 8]> = self.shape.roots.iter().copied().collect();

        // Per-evaluation scratch, indexed by child: `None` not evaluated (yet, or at all), else
        // EACH child's own output slots (a child may itself be a multi-output composite).
        scratch.resize(n, None);

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
            let walk = {
                // The slot feeding an input port, borrowed where it is kept: `None` while its
                // source child is unevaluated.
                let at = |p: usize| match incoming.source(ci, p) {
                    Source::Input(fp) => Some(In::of_value(inputs.get(fp as usize).and_then(|v| v.as_deref()))),
                    Source::Child(fc, fp) => self.output(fc as usize, fp as usize, scratch, inputs),
                    Source::Unwired | Source::Missing(..) => Some(In::Absent),
                };
                // A choice pulls what it needs; anything else, every input not yet evaluated. Its
                // inputs are gathered in the same pass that looks for one not yet evaluated.
                let mut gathered: Refs = SmallVec::new();
                let mut pending = false;
                match kind.choice.map(|which| choose(which, at)) {
                    Some(Chosen::Need(p)) => Walk::Need(p),
                    // A value read at a port, to be copied or given once nothing is borrowed.
                    Some(Chosen::Done(In::Value(_), Some(p))) => Walk::Chosen(p),
                    Some(Chosen::Done(slot, _)) => Walk::Decided(slot.owned()),
                    None => match (0..k).find(|&p| match at(p) {
                        None => true,
                        Some(slot) => {
                            pending |= slot == In::Pending;
                            gathered.push(slot.value());
                            false
                        }
                    }) {
                        Some(p) => Walk::Need(p),
                        None if !pending && self.giving(ci, &gathered, scratch, inputs) => Walk::Giving,
                        None => Walk::Fired(self.fire(ci, &mut gathered, pending, &open, resolver, observer, path)),
                    },
                }
            };
            let outs = match walk {
                Walk::Need(p) => {
                    let (fc, _) = incoming.get(ci as u32, p as u32).expect("only a wired port is unevaluated");
                    stack.push(fc as usize);
                    continue;
                }
                Walk::Chosen(p) => {
                    let outs = self.chosen(ci, p, scratch, inputs, true, !observer.reads_produced());
                    self.decided(ci, outs, observer, path)
                }
                Walk::Decided(slot) => self.decided(ci, smallvec![slot], observer, path),
                Walk::Giving => self.fire_giving(ci, scratch, inputs, true, resolver, observer, path),
                Walk::Fired(outs) => outs,
            };
            stack.pop();
            scratch[ci] = Some(outs);
        }

        self.outputs(scratch, n_outputs, inputs)
    }

    /// A choice's outputs once it has decided, told to the observer as a child's are.
    fn decided(&self, ci: usize, outs: Slots, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slots {
        self.told(ci, observer, path, |_, _| outs)
    }

    /// The composite's outputs, from the children's slots in `scratch` (moved out where only one
    /// output reads one) or its own inputs (moved out likewise when given). Apart from the walks,
    /// so the recursion through them doesn't carry this frame.
    #[inline(never)]
    fn outputs(&self, scratch: &mut [Option<Slots>], n_outputs: usize, inputs: &mut [Option<Cow<'_, Value>>]) -> Slots {
        let incoming = &self.shape.incoming;
        // Each exposed output is whatever feeds that boundary-output port: a child's output at the
        // right port, or — symmetric with the child-input case above — a promoted input routed
        // straight through. (A composite whose output *is* one of its inputs is a valid,
        // validate-accepted shape; the BOUNDARY→BOUNDARY arm below is why that doesn't panic.)
        // The scratch goes when this returns, and the run is done with its inputs, so a value only
        // one exposed output reads is moved out, not copied. An output fed by a choice that lends
        // is fed by where the value is kept, so two outputs reading one value through different
        // choices are seen to share it.
        let place = |op: u32| {
            let mut at = incoming.get(BOUNDARY, op)?;
            while at.0 != BOUNDARY && at.1 == 0 && (at.0 as usize) < self.kinds.len() && self.lends(at.0 as usize, scratch) {
                let c = at.0 as usize;
                at = match incoming.source(c, self.lent_port(c, scratch, inputs)) {
                    Source::Input(fp) => (BOUNDARY, fp),
                    Source::Child(fc, fp) => (fc, fp),
                    Source::Unwired | Source::Missing(..) => return None,
                };
            }
            Some(at)
        };
        // The value at `at`, moved out unless another output reads it there too.
        fn slot_at(at: Option<(u32, u32)>, shared: bool, scratch: &mut [Option<Slots>], inputs: &mut [Option<Cow<'_, Value>>]) -> Slot {
            match at {
                Some((BOUNDARY, fp)) => match inputs.get_mut(fp as usize) {
                    Some(given @ Some(Cow::Owned(_))) if !shared => Slot::of(given.take().map(Cow::into_owned)),
                    Some(Some(v)) => Slot::Value(v.as_ref().clone()),
                    _ => Slot::Absent,
                },
                Some((fc, fp)) => match scratch.get_mut(fc as usize).and_then(|o| o.as_mut()).and_then(|o| o.get_mut(fp as usize)) {
                    Some(slot) if !shared => std::mem::replace(slot, Slot::Absent),
                    Some(slot) => slot.clone(),
                    None => Slot::Absent,
                },
                None => Slot::Absent,
            }
        }
        if n_outputs == 1 {
            // One output, as most are: nothing else reads its value, and no list of places is
            // made for each run (a `map`'s every item).
            let at = place(0);
            return smallvec![slot_at(at, false, scratch, inputs)];
        }
        let places: SmallVec<[Option<(u32, u32)>; 2]> = (0..n_outputs as u32).map(place).collect();
        (0..n_outputs)
            .map(|op| {
                let shared = places.iter().enumerate().any(|(other, at)| other != op && at.is_some() && *at == places[op]);
                slot_at(places[op], shared, scratch, inputs)
            })
            .collect()
    }
}

/// What the plan does with a child once it has looked at its inputs.
enum Fired {
    Done(Slots),
    /// It may be given some of them: fired once nothing is borrowed.
    Giving,
    /// An input isn't evaluated yet: the walk carries on.
    Ahead,
}

/// What the walk does with a child once it has looked at its inputs.
enum Walk {
    /// Evaluate what feeds this port first.
    Need(usize),
    /// A choice decided on the value at this port.
    Chosen(usize),
    /// A choice decided on this.
    Decided(Slot),
    /// It may be given some of its inputs: fired once nothing is borrowed.
    Giving,
    Fired(Slots),
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
fn eval_map(inputs: &mut [Option<Cow<'_, Value>>], params: &[Param], resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slot {
    let len = match inputs.first() {
        Some(Some(list)) => match list.as_ref() {
            Value::List(xs) => xs.len(),
            _ => return Slot::Absent,
        },
        _ => return Slot::Absent,
    };
    if !present(&inputs[1..]) {
        return Slot::Absent;
    }
    let Some(unit) = function(params, resolver) else { return Slot::Absent };
    let unit = Ready::of(unit, resolver);

    let mut out = Vec::with_capacity(len);
    let (mut pending, mut absent) = (false, false);
    let mut asks = Asks(observer);
    // Every item is applied, even after one comes out absent, so each item's requests are asked
    // in the same round rather than one round per item. A result goes a level down, into the
    // list: one already as deep as a value may nest makes the map give nothing
    // (`value_hash::MAX_VALUE_DEPTH`), as a leaf that would put it there does.
    let mut tally = |slot: Slot| match slot {
        Slot::Value(v) if nests_within(&v, MAX_VALUE_DEPTH - 1) => out.push(v),
        Slot::Value(_) | Slot::Absent => absent = true,
        Slot::Pending => pending = true,
    };
    // The item and the extras, lent: an application copies only what it keeps. Made once, the
    // item put in place for each. A list given to the map gives each item to its application.
    let (list, extra) = inputs.split_at_mut(1);
    match (&unit, &list[0]) {
        // A leaf over items lent, nothing to give it: by reference, as `eval_as` reads them, which
        // saves a tenth of a leaf's application.
        (Ready::Other(leaf, kind), Some(Cow::Borrowed(Value::List(xs)))) => {
            let mut refs: Refs = SmallVec::with_capacity(1 + extra.len());
            refs.push(None);
            refs.extend(extra.iter().map(|v| v.as_deref()));
            for (k, x) in xs.iter().enumerate() {
                refs[0] = Some(x);
                tally(at_item(k, path, |path| eval_as(leaf, kind, &refs, resolver, &mut asks, path)));
            }
        }
        // A composite over items lent: run, each item put in place.
        (Ready::Composite(unit, Some(prepared)), Some(Cow::Borrowed(Value::List(xs)))) if !crate::takes_trigger(unit) => {
            let mut args: Args = SmallVec::with_capacity(1 + extra.len());
            args.push(None);
            args.extend(extra.iter().map(|v| v.as_deref().map(Cow::Borrowed)));
            for (k, x) in xs.iter().enumerate() {
                args[0] = Some(Cow::Borrowed(x));
                tally(at_item(k, path, |path| prepared.run(unit.outputs.len(), &mut args, resolver, &mut asks, path)));
            }
        }
        _ => {
            let mut args: Args = SmallVec::with_capacity(1 + extra.len());
            args.push(None);
            args.extend(extra.iter().map(|v| v.as_deref().map(Cow::Borrowed)));
            let Some(items) = Items::of(list[0].take()) else { return Slot::Absent };
            for (k, item) in items.enumerate() {
                args[0] = Some(item);
                tally(apply(&unit, &mut args, resolver, &mut asks, path, k));
            }
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

/// A higher-order leaf that tests each item of its list (input 0) with the unit its `fn` param
/// names, the inputs after the list passed after the item as `map` passes them (`fn(item,
/// extra…)`), its answer read as JavaScript's ToBoolean ([`lists::truthy`]). `filter` applies
/// every item, as `map` does, and keeps those whose answer is true. The others stop at the item
/// that decides their answer (`any` at the first true, `all` at the first false, the `find` family
/// at the first match, `find_last` and `find_last_index` scanning from the end) and apply nothing
/// after it: an answer they never needed may be a request the host would perform.
///
/// An application that gives nothing, or waits, makes the leaf give nothing, or wait; a stopping
/// leaf stops there as well, since whether it would have gone on depends on that answer. Each
/// application asks at the node's path then the item's position, as `map`'s do. The items are
/// lent to the function; a list given to `filter`, `find` or `find_last` moves the items it
/// answers with rather than copying them.
fn eval_test(test: Test, inputs: &mut [Option<Cow<'_, Value>>], params: &[Param], resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slot {
    if !matches!(inputs.first(), Some(Some(list)) if matches!(list.as_ref(), Value::List(_))) || !present(&inputs[1..]) {
        return Slot::Absent;
    }
    let Some(unit) = function(params, resolver) else { return Slot::Absent };
    let unit = Ready::of(unit, resolver);
    let (list, extra) = inputs.split_at_mut(1);
    let Some(items) = Items::of(take_given(&mut list[0])) else { return Slot::Absent };
    let mut asks = Asks(observer);
    // Item `k`'s answer: true or false, or the slot that stops the leaf.
    let mut answer = |k: usize, item: &Value| {
        let mut args: Args = SmallVec::with_capacity(1 + extra.len());
        args.push(Some(Cow::Borrowed(item)));
        args.extend(extra.iter().map(|v| v.as_deref().map(Cow::Borrowed)));
        match apply(&unit, &mut args, resolver, &mut asks, path, k) {
            Slot::Value(v) => Ok(lists::truthy(&v)),
            other => Err(other),
        }
    };
    match test {
        Test::Filter => {
            let (mut kept, mut pending, mut absent) = (Vec::new(), false, false);
            // Every item is applied, even after one comes out absent, so each item's requests are
            // asked in the same round, as `map` asks them.
            for (k, item) in items.enumerate() {
                match answer(k, &item) {
                    Ok(true) => kept.push(item.into_owned()),
                    Ok(false) => {}
                    Err(Slot::Pending) => pending = true,
                    Err(_) => absent = true,
                }
            }
            if pending {
                Slot::Pending
            } else if absent {
                Slot::Absent
            } else {
                Slot::Value(Value::List(kept))
            }
        }
        Test::Any | Test::All => {
            // The answer that decides: `any` stops at a true, `all` at a false.
            let decides = test == Test::Any;
            for (k, item) in items.enumerate() {
                match answer(k, &item) {
                    Ok(b) if b == decides => return Slot::Value(Value::Boolean(decides)),
                    Ok(_) => {}
                    Err(stop) => return stop,
                }
            }
            Slot::Value(Value::Boolean(!decides))
        }
        Test::Find | Test::FindIndex | Test::FindLast | Test::FindLastIndex => {
            let backward = matches!(test, Test::FindLast | Test::FindLastIndex);
            let mut items = items.enumerate();
            while let Some((k, item)) = if backward { items.next_back() } else { items.next() } {
                match answer(k, &item) {
                    Ok(true) if test.keeps_items() => return Slot::Value(item.into_owned()),
                    Ok(true) => return Slot::Value(Value::Number(k as f64)),
                    Ok(false) => {}
                    Err(stop) => return stop,
                }
            }
            if test.keeps_items() { Slot::Absent } else { Slot::Value(Value::Number(-1.0)) }
        }
    }
}

/// Higher-order `sort_by`: the items of its list (input 0) in the order the unit its `fn` param
/// names puts them, the inputs after the list passed after the two items (`fn(a, b, extra…)`). Its
/// answer is a number: below 0 puts `a` first, above 0 `b`, `0` and `-0` keep their order. Stable,
/// by one pinned merge sort ([`lists::merge_sort`]), so a comparator that isn't a total order
/// still gives one deterministic order, where Rust's own sort may panic.
///
/// An answer that isn't a number, or none, makes the leaf give nothing, and one that waits makes it
/// wait. Either stops it, since which items are compared next depends on that answer. The `k`th
/// comparison asks at the node's path then `k`. The items are lent to the function, and a list given
/// to the leaf is reordered without copying its items.
fn eval_sort_by(inputs: &mut [Option<Cow<'_, Value>>], params: &[Param], resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slot {
    if !matches!(inputs.first(), Some(Some(list)) if matches!(list.as_ref(), Value::List(_))) || !present(&inputs[1..]) {
        return Slot::Absent;
    }
    let Some(unit) = function(params, resolver) else { return Slot::Absent };
    let unit = Ready::of(unit, resolver);
    let (list, extra) = inputs.split_at_mut(1);
    let list = take_given(&mut list[0]).expect("checked present");
    let mut asks = Asks(observer);
    let order = {
        let Value::List(xs) = list.as_ref() else { unreachable!("checked to be a list") };
        let mut compared = 0;
        lists::merge_sort(xs.len(), |a, b| {
            let mut args: Args = SmallVec::with_capacity(2 + extra.len());
            args.push(Some(Cow::Borrowed(&xs[a])));
            args.push(Some(Cow::Borrowed(&xs[b])));
            args.extend(extra.iter().map(|v| v.as_deref().map(Cow::Borrowed)));
            compared += 1;
            match apply(&unit, &mut args, resolver, &mut asks, path, compared - 1) {
                Slot::Value(Value::Number(n)) => Ok(n > 0.0),
                Slot::Pending => Err(Slot::Pending),
                _ => Err(Slot::Absent),
            }
        })
    };
    let order = match order {
        Ok(order) => order,
        Err(stop) => return stop,
    };
    Slot::Value(Value::List(match list {
        Cow::Owned(Value::List(xs)) => {
            let mut xs: Vec<Option<Value>> = xs.into_iter().map(Some).collect();
            order.into_iter().map(|k| xs[k].take().expect("each item once")).collect()
        }
        Cow::Borrowed(Value::List(xs)) => order.into_iter().map(|k| xs[k].clone()).collect(),
        _ => unreachable!("checked to be a list"),
    }))
}

/// Higher-order `group_by`: the items of its list (input 0) in lists by the key the unit its `fn`
/// param names answers for each, the inputs after the list passed after the item as `map` passes
/// them (`fn(item, extra…)`). Each list keeps the items' order, and the groups come in key order
/// ([`records::grouped`]).
///
/// Every item is applied, as `map` applies them, so each item's requests are asked in the same
/// round. An answer that isn't a text (JavaScript converts a number) or none makes the leaf give
/// nothing, and one that waits makes it wait, waiting winning. The items are lent to the function;
/// a list given to the leaf moves its items into their groups rather than copying them.
fn eval_group_by(inputs: &mut [Option<Cow<'_, Value>>], params: &[Param], resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slot {
    // The items go a level down, each into its group's list.
    if !matches!(inputs.first(), Some(Some(list)) if matches!(list.as_ref(), Value::List(_)) && nests_within(list, MAX_VALUE_DEPTH - 1)) || !present(&inputs[1..]) {
        return Slot::Absent;
    }
    let Some(unit) = function(params, resolver) else { return Slot::Absent };
    let unit = Ready::of(unit, resolver);
    let (list, extra) = inputs.split_at_mut(1);
    let Some(items) = Items::of(take_given(&mut list[0])) else { return Slot::Absent };
    let mut asks = Asks(observer);
    let (mut groups, mut pending, mut absent) = (std::collections::BTreeMap::<String, Vec<Value>>::new(), false, false);
    for (k, item) in items.enumerate() {
        let key = {
            let mut args: Args = SmallVec::with_capacity(1 + extra.len());
            args.push(Some(Cow::Borrowed(&*item)));
            args.extend(extra.iter().map(|v| v.as_deref().map(Cow::Borrowed)));
            apply(&unit, &mut args, resolver, &mut asks, path, k)
        };
        match key {
            Slot::Value(Value::Text(key)) => groups.entry(key).or_default().push(item.into_owned()),
            Slot::Pending => pending = true,
            _ => absent = true,
        }
    }
    if pending {
        Slot::Pending
    } else if absent {
        Slot::Absent
    } else {
        Slot::Value(records::grouped(groups))
    }
}

/// `json_parse` with a reviver and `json_stringify` with a replacer: the unit the `fn` param names,
/// applied to a key and a value (`fn(key, value, extra…)`, the inputs after the first passed after
/// the value), bottom-up over the parsed text ([`crate::json::revive`]) or top-down over the value
/// before it is written ([`crate::json::replace`], then laid out by `indent`). What it answers
/// nothing for is left out (`null` in a list), and nothing for the whole value makes the leaf give
/// nothing, as JavaScript's `undefined` does. A text that isn't JSON, a value with no JSON form, or
/// `keys` set beside `fn` gives nothing.
///
/// An application that waits makes the leaf wait, and stops it there: what is applied next depends
/// on that answer (a reviver's answer is part of the value its holder's application is given, a
/// replacer's decides which members are visited). The `k`th application asks at the node's path
/// then `k`, in the walk's order.
fn eval_json(json: Json, inputs: &mut [Option<Cow<'_, Value>>], params: &[Param], resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slot {
    if !matches!(inputs.first(), Some(Some(_))) || !present(&inputs[1..]) {
        return Slot::Absent;
    }
    let param = |name: &str| params.iter().find(|p| p.name() == name).and_then(|p| p.value()).unwrap_or("");
    if json == Json::Stringify && !param("keys").is_empty() {
        return Slot::Absent;
    }
    let Some(unit) = function(params, resolver) else { return Slot::Absent };
    let unit = Ready::of(unit, resolver);
    let (first, extra) = inputs.split_at_mut(1);
    let mut asks = Asks(observer);
    let mut applied = 0;
    let mut answer = |key: &str, value: &Value| -> Result<Option<Value>, Slot> {
        let key = Value::Text(key.to_string());
        let mut args: Args = SmallVec::with_capacity(2 + extra.len());
        args.push(Some(Cow::Borrowed(&key)));
        args.push(Some(Cow::Borrowed(value)));
        args.extend(extra.iter().map(|v| v.as_deref().map(Cow::Borrowed)));
        applied += 1;
        match apply(&unit, &mut args, resolver, &mut asks, path, applied - 1) {
            Slot::Value(v) => Ok(Some(v)),
            Slot::Absent => Ok(None),
            Slot::Pending => Err(Slot::Pending),
        }
    };
    let first = first[0].as_deref().expect("checked present");
    let done = match json {
        Json::Parse => {
            let Value::Text(text) = first else { return Slot::Absent };
            let Some(parsed) = crate::json::parse(text) else { return Slot::Absent };
            // Each answer stands where its value was, as deep as 64: what it built nests within
            // the bound, or nothing.
            crate::json::revive(parsed, &mut answer).map(|v| v.filter(|v| nests_within(v, MAX_VALUE_DEPTH)))
        }
        Json::Stringify => crate::json::replace(first, &mut answer).map(|replaced| {
            replaced.and_then(|v| crate::json::stringify_laid_out(&v, &crate::json::Layout { gap: crate::json::gap(param("indent")), keys: None })).map(Value::Text)
        }),
    };
    match done {
        Ok(Some(v)) => Slot::Value(v),
        Ok(None) => Slot::Absent,
        Err(stop) => stop,
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
    /// Anything else, evaluated as it always is, what it is decided once.
    Other(&'r Unit, NodeKind),
}

impl<'r> Ready<'r> {
    fn of(unit: &'r Unit, resolver: &'r impl Resolver) -> Ready<'r> {
        match &unit.body {
            UnitBody::Composite { children, edges } => Ready::Composite(unit, Prepared::of(unit, children, edges, resolver).map(|p| Box::new(p.planned()))),
            _ => Ready::Other(unit, NodeKind::of(unit)),
        }
    }
}

/// One application of a `map`/`fold` function, to item `k`: evaluated at the node's path then
/// `k`, its one output (or absent, as [`eval_one`]). A composite runs as `eval_at` would run it.
/// What `args` gives (a `fold`'s accumulator), the function may build on.
fn apply<'r>(unit: &Ready<'r>, args: &mut Args<'_>, resolver: &'r impl Resolver, asks: &mut Asks, path: &mut Vec<u32>, k: usize) -> Slot {
    at_item(k, path, |path| match unit {
        Ready::Other(unit, kind) if args.iter().any(|a| matches!(a, Some(Cow::Owned(v)) if heavy(v))) => eval_given(unit, kind, args, resolver, asks, path),
        Ready::Other(unit, kind) => eval_as(unit, kind, &refs(args), resolver, asks, path),
        Ready::Composite(unit, prepared) => {
            // As `eval_at`: a sink's trigger only has to be there.
            let n = if crate::takes_trigger(unit) && args.len() == unit.inputs.len() { args.len() - 1 } else { args.len() };
            match prepared {
                Some(prepared) => prepared.run(unit.outputs.len(), &mut args[..n], resolver, asks, path),
                None => smallvec![Slot::Absent; unit.outputs.len()],
            }
        }
    })
}

/// What `eval` makes of item `k`'s application: evaluated at the node's path then `k`, its one
/// output (or absent, as [`eval_one`]).
fn at_item(k: usize, path: &mut Vec<u32>, eval: impl FnOnce(&mut Vec<u32>) -> Slots) -> Slot {
    let Ok(k) = u32::try_from(k) else { return Slot::Absent };
    path.push(k);
    let mut outs = eval(path);
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
    fn reads_produced(&self) -> bool {
        false
    }
}

/// Whether every one of the inputs is present.
fn present(inputs: &[Option<Cow<'_, Value>>]) -> bool {
    inputs.iter().all(Option::is_some)
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
///
/// The accumulator is given to each application, not lent: nothing else reads it, so a function
/// that grows it (`[...acc, x]`, `insert(acc, k, v)`) grows it in place, one step O(1) where
/// copying it made the fold O(n²).
fn eval_fold(inputs: &mut [Option<Cow<'_, Value>>], params: &[Param], resolver: &impl Resolver, observer: &mut impl Observer, path: &mut Vec<u32>) -> Slot {
    if !matches!(inputs.first(), Some(Some(_))) || !matches!(inputs.get(1), Some(Some(list)) if matches!(list.as_ref(), Value::List(_))) {
        return Slot::Absent;
    }
    if !present(inputs.get(2..).unwrap_or_default()) {
        return Slot::Absent;
    }
    let Some(unit) = function(params, resolver) else { return Slot::Absent };
    let unit = Ready::of(unit, resolver);

    let (head, extra) = inputs.split_at_mut(2);
    let mut acc = take_given(&mut head[0]).map(Cow::into_owned).expect("checked present");
    let mut asks = Asks(observer);
    // A list given to the fold gives each item to its application too.
    let Some(items) = Items::of(take_given(&mut head[1])) else { return Slot::Absent };
    for (k, elem) in items.enumerate() {
        // fn(acc, elem, extra…) → new acc; absent or pending ⇒ the whole fold is.
        let next = match &unit {
            // A leaf, and nothing worth giving it (a sum's number): by reference, as `eval_as`
            // reads them, which saves a quarter of a step.
            Ready::Other(leaf, kind) if !heavy(&acc) && !matches!(&elem, Cow::Owned(v) if heavy(v)) => {
                let mut refs: Refs = SmallVec::with_capacity(2 + extra.len());
                refs.push(Some(&acc));
                refs.push(Some(&*elem));
                refs.extend(extra.iter().map(|v| v.as_deref()));
                at_item(k, path, |path| eval_as(leaf, kind, &refs, resolver, &mut asks, path))
            }
            _ => {
                let mut args: Args = SmallVec::with_capacity(2 + extra.len());
                args.push(Some(Cow::Owned(acc)));
                args.push(Some(elem));
                args.extend(extra.iter().map(|v| v.as_deref().map(Cow::Borrowed)));
                apply(&unit, &mut args, resolver, &mut asks, path, k)
            }
        };
        acc = match next {
            Slot::Value(v) => v,
            other => return other,
        };
    }
    Slot::Value(acc)
}

/// An input moved out when it was given, as nothing else reads it; a lent one is lent again and
/// left in its slot, as a `map` hands the same extras to each item's application (a `fold` that is
/// a `map`'s function gets the map's extra as its list, at every item).
fn take_given<'v>(slot: &mut Option<Cow<'v, Value>>) -> Option<Cow<'v, Value>> {
    match slot {
        Some(Cow::Borrowed(v)) => Some(Cow::Borrowed(*v)),
        _ => slot.take(),
    }
}

/// The items of a higher-order leaf's list: each given when the list was, else lent.
enum Items<'v> {
    Given(std::vec::IntoIter<Value>),
    Lent(std::slice::Iter<'v, Value>),
}

impl<'v> Items<'v> {
    /// The items of `list`, when it is one.
    fn of(list: Option<Cow<'v, Value>>) -> Option<Items<'v>> {
        match list? {
            Cow::Owned(Value::List(xs)) => Some(Items::Given(xs.into_iter())),
            Cow::Borrowed(Value::List(xs)) => Some(Items::Lent(xs.iter())),
            _ => None,
        }
    }
}

impl<'v> Iterator for Items<'v> {
    type Item = Cow<'v, Value>;
    fn next(&mut self) -> Option<Cow<'v, Value>> {
        match self {
            Items::Given(xs) => xs.next().map(Cow::Owned),
            Items::Lent(xs) => xs.next().map(Cow::Borrowed),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Items::Given(xs) => xs.size_hint(),
            Items::Lent(xs) => xs.size_hint(),
        }
    }
}

/// From the end, for `find_last` and `find_last_index`.
impl<'v> DoubleEndedIterator for Items<'v> {
    fn next_back(&mut self) -> Option<Cow<'v, Value>> {
        match self {
            Items::Given(xs) => xs.next_back().map(Cow::Owned),
            Items::Lent(xs) => xs.next_back().map(Cow::Borrowed),
        }
    }
}

impl ExactSizeIterator for Items<'_> {}

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
        let walked_out = walk.run(n_out, &mut lent(&refs), r, &mut walked, &mut walked_path);
        let plan = Prepared::of(unit, children, edges, r).unwrap().planned();
        assert!(plan.plan.is_some(), "no choice but pass-throughs: it has a plan");
        for _ in 0..2 {
            let (mut ran, mut ran_path) = (observer(), Vec::new());
            let ran_out = plan.run(n_out, &mut lent(&refs), r, &mut ran, &mut ran_path);
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

    /// A resolver that keeps each composite's children resolved, as the server's does (holon's
    /// `units.rs`): every unit `r` holds, shared.
    struct Keeping {
        units: BTreeMap<Hash, Arc<Unit>>,
        shapes: Shapes,
    }

    impl Keeping {
        fn over(r: &MapResolver) -> Keeping {
            let units: BTreeMap<Hash, Arc<Unit>> = r.units().map(|u| (crate::hash_unit(u).unwrap(), Arc::new(u.clone()))).collect();
            let shapes = Shapes::kept(units.values(), |h| units.get(h));
            Keeping { units, shapes }
        }
    }

    impl Resolver for Keeping {
        fn resolve(&self, hash: &Hash) -> Option<&Unit> {
            self.units.get(hash).map(|u| u.as_ref())
        }
        fn shape(&self, unit: &Unit) -> Option<&Shape> {
            self.shapes.get(unit)
        }
        fn resolved(&self, unit: &Unit) -> Option<&Resolved> {
            self.shapes.resolved(unit)
        }
    }

    /// Children the resolver keeps ([`Shapes::kept`]) give what resolving them for each run gives,
    /// on 3,000 random composites, each run at the top, and inside another composite beside a
    /// `map` it is the function of: the same events in the same order, and the same outputs. A
    /// composite one of whose children doesn't resolve keeps its shape alone.
    #[test]
    fn children_the_resolver_keeps_give_what_resolving_them_gives() {
        let mut r = MapResolver::new();
        let mut units = Vec::new();
        random_composites(&mut r, 3000, |u, _, _, _, _| units.push(u.clone()));
        let mut roots = Vec::new();
        for unit in units {
            // Twin children no edge tells apart don't hash: such a unit is never stored.
            let Ok(inner) = r.insert(unit) else { continue };
            let map = r.insert(over("map", &inner, 1)).unwrap();
            let outer = multi_composite(
                vec![Port { name: "xs".into(), ty: Type::list_any() }, nport("b")],
                vec![Type::Any, Type::Number],
                vec![map, inner],
                vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(BOUNDARY, 1, 1, 0), edge(BOUNDARY, 1, 1, 1), edge(0, 0, BOUNDARY, 0), edge(1, 0, BOUNDARY, 1)],
            );
            roots.push((inner, r.insert(outer).unwrap()));
        }
        let keeping = Keeping::over(&r);
        let mut kept = 0;
        for (inner, outer) in &roots {
            for (h, inputs) in [(inner, vec![Some(num(3.0)), Some(num(0.0))]), (outer, vec![Some(list(vec![num(1.0), num(-2.0), num(0.0)])), Some(num(4.0))])] {
                let unit = keeping.resolve(h).unwrap();
                kept += keeping.resolved(unit).is_some() as usize;
                let (mut want, mut got) = (Events::default(), Events::default());
                let want_out = eval_observed(r.resolve(h).unwrap(), &inputs, &r, &mut want);
                assert_eq!(eval_observed(unit, &inputs, &keeping, &mut got), want_out);
                assert_eq!(got.0, want.0);
            }
        }
        assert!(roots.len() > 2000, "composites stored: {}", roots.len());
        assert_eq!(kept, 2 * roots.len(), "every composite's children kept");
        let lost = Arc::new(composite(vec![], Type::Number, vec![[7; 32]], vec![edge(0, 0, BOUNDARY, 0)]));
        let shapes = Shapes::kept([&lost], |_| None);
        assert!(shapes.get(&lost).is_some() && shapes.resolved(&lost).is_none(), "a child that doesn't resolve: its shape alone");
    }

    /// A leaf the host sources ([`crate::SOURCED`]) produces what the resolver supplies for it, as
    /// it is: at the top of a composite whose children are kept and inside a `map`'s function, a
    /// different value each run over the same kept children; with nothing supplied, nothing. The
    /// eager oracle agrees.
    #[test]
    fn a_sourced_leaf_produces_what_the_resolver_supplies() {
        struct Supplying<'a> {
            keeping: &'a Keeping,
            value: Option<Value>,
        }
        impl Resolver for Supplying<'_> {
            fn resolve(&self, hash: &Hash) -> Option<&Unit> {
                self.keeping.resolve(hash)
            }
            fn shape(&self, unit: &Unit) -> Option<&Shape> {
                self.keeping.shape(unit)
            }
            fn resolved(&self, unit: &Unit) -> Option<&Resolved> {
                self.keeping.resolved(unit)
            }
            fn sourced(&self, unit: &Unit) -> Option<&Value> {
                self.value.as_ref().filter(|_| matches!(&unit.body, UnitBody::Primitive { name } if name == "store_read"))
            }
        }
        let mut r = MapResolver::new();
        let read = r.insert(Unit { params: vec![Param::frozen("key", "k")], ..crate::interface_of("store_read").unwrap() }).unwrap();
        let pair = r.insert(prim("list", 2, Type::list_any())).unwrap();
        // f(x) = [x, read]; root(xs) = (map(f, xs), read)
        let f = r.insert(composite(vec![nport("x")], Type::list_any(), vec![read, pair], vec![edge(BOUNDARY, 0, 1, 0), edge(0, 0, 1, 1), edge(1, 0, BOUNDARY, 0)])).unwrap();
        let map = r.insert(over("map", &f, 0)).unwrap();
        let root = r
            .insert(multi_composite(
                vec![Port { name: "xs".into(), ty: Type::list_any() }],
                vec![Type::Any, Type::Any],
                vec![map, read],
                vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, BOUNDARY, 0), edge(1, 0, BOUNDARY, 1)],
            ))
            .unwrap();
        let keeping = Keeping::over(&r);
        let root = keeping.resolve(&root).unwrap();
        let inputs = [Some(list(vec![num(1.0), num(2.0)]))];
        let held = Value::Record(BTreeMap::from([("n".to_string(), num(5.0)), ("tags".to_string(), list(vec![txt("a")]))]));
        for value in [held, num(3.0), list(vec![])] {
            let supplying = Supplying { keeping: &keeping, value: Some(value.clone()) };
            assert!(supplying.resolved(root).is_some(), "kept");
            let want = vec![Some(list(vec![list(vec![num(1.0), value.clone()]), list(vec![num(2.0), value.clone()])])), Some(value)];
            assert_eq!(eval(root, &inputs, &supplying).into_vec(), want);
            assert_eq!(crate::eager::eval_eager(root, &inputs, &supplying), want, "eager agrees");
        }
        let nothing = Supplying { keeping: &keeping, value: None };
        assert_eq!(eval(root, &inputs, &nothing)[1], None);
        assert_eq!(eval(root, &inputs, &nothing).into_vec(), crate::eager::eval_eager(root, &inputs, &nothing));
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
            let walked = Prepared::of(unit, children, edges, &r).unwrap().run(*n_out, &mut lent(&refs), &r, &mut NoObserver, &mut Vec::new());
            let mut plan = Prepared::of(unit, children, edges, &r).unwrap().planned();
            let order = &mut plan.plan.as_mut().unwrap().order;
            let before = order.clone();
            order.reverse();
            handed += usize::from(*order != before);
            assert_eq!(plan.run(*n_out, &mut lent(&refs), &r, &mut NoObserver, &mut Vec::new()), walked, "edges {edges:?}");
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
                let free = prepared.run(*n_out, &mut lent(&refs), &r, &mut NoObserver, &mut Vec::new());
                let _taken = prepared.scratch.borrow_mut();
                assert_eq!(prepared.run(*n_out, &mut lent(&refs), &r, &mut NoObserver, &mut Vec::new()), free, "edges {edges:?}");
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
    /// a request the host answers, a higher-order leaf, or a source whose value the host supplies
    /// ([`crate::SOURCED`]: every capability it has is sourced, and it asks nothing). And every
    /// opcode is a genesis primitive's, its name finding it. A primitive added without its
    /// computation would otherwise read as one that is always absent.
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
            assert_eq!(crate::SOURCED.contains(name), sourced && Request::of(name).is_none(), "{name}: SOURCED is the sources the host supplies");
            assert!(
                Op::of(name).is_some() || Request::of(name).is_some() || crate::PASSES_EXTRA.contains(name) || crate::SOURCED.contains(name),
                "{name} is computed nowhere: no opcode, no request, and not a source the host supplies"
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
    fn maps_nested_as_deep_as_a_host_runs_fit_half_a_threads_stack() {
        // The same bound through `map`s, each function mapping the next over its item: the
        // recursion that goes through a function's preparation, its giving and `eval_map` at
        // every level, the deepest per level there is (docs/engineering/themes/js-library-races/leaves.md).
        // 32 maps, each with its function, are 64 units deep.
        let mut r = MapResolver::new();
        let item = || Port { name: "item".into(), ty: Type::Any };
        let mut f = r.insert(composite(vec![item()], Type::Any, vec![], vec![edge(BOUNDARY, 0, BOUNDARY, 0)])).unwrap();
        for _ in 0..31 {
            let map = r.insert(map_unit(&f)).unwrap();
            f = r.insert(composite(vec![item()], Type::Any, vec![map], vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, BOUNDARY, 0)])).unwrap();
        }
        let top = map_unit(&f);
        // A list nested as deep, so every level has a list to map.
        let mut list = num(1.0);
        for _ in 0..32 {
            list = Value::List(vec![list]);
        }
        let want = list.clone();
        let out = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(move || eval(&top, &[Some(list)], &r).into_vec())
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(out, vec![Some(want)]);
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

    /// A higher-order leaf other than `map` and `fold` over `fn_hash`: its list, then `extras`
    /// inputs passed to the function after the item.
    fn over(name: &str, fn_hash: &Hash, extras: usize) -> Unit {
        let mut inputs = vec![Port { name: "list".into(), ty: Type::list_any() }];
        inputs.extend((0..extras).map(|i| Port { name: format!("extra{i}"), ty: Type::Any }));
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![Param::frozen("fn", hex(fn_hash))],
            inputs,
            outputs: vec![Port { name: "out".into(), ty: Type::Any }],
        }
    }

    /// The leaves that test each item read their function's answer as JavaScript's ToBoolean,
    /// hand it the inputs after the list, and give what the eager oracle gives: on the list lent,
    /// and given to them inside a composite.
    #[test]
    fn the_testing_leaves_read_their_functions_answer_as_javascript_does() {
        use crate::eager::eval_eager;
        let mut r = MapResolver::new();
        // same(x) = x: each item is its own answer.
        let same = r.insert(Unit { params: vec![Param::frozen("type", "Number"), Param::frozen("value", "0")], ..prim("default", 1, Type::Any) }).unwrap();
        let empty = Value::Record(BTreeMap::new());
        let items = list(vec![num(0.0), num(-0.0), txt(""), Value::Boolean(false), list(vec![]), empty.clone(), txt("a"), num(1.0), Value::Boolean(true)]);
        let cases: [(&str, Option<Value>, Option<Value>); 7] = [
            ("filter", Some(list(vec![list(vec![]), empty, txt("a"), num(1.0), Value::Boolean(true)])), Some(list(vec![]))),
            ("any", Some(Value::Boolean(true)), Some(Value::Boolean(false))),
            ("all", Some(Value::Boolean(false)), Some(Value::Boolean(true))),
            ("find", Some(list(vec![])), None),
            ("find_index", Some(num(4.0)), Some(num(-1.0))),
            ("find_last", Some(Value::Boolean(true)), None),
            ("find_last_index", Some(num(8.0)), Some(num(-1.0))),
        ];
        for (name, over_items, over_none) in cases {
            let leaf = over(name, &same, 0);
            for (xs, want) in [(items.clone(), over_items), (list(vec![]), over_none)] {
                assert_eq!(eval(&leaf, &[Some(xs.clone())], &r).into_vec(), vec![want.clone()], "{name} on {xs:?}");
                assert_eq!(eval_eager(&leaf, &[Some(xs.clone())], &r), vec![want.clone()], "{name}, eagerly");
                // Inside a composite, the list is built there and given to the leaf.
                let hlist = r.insert(prim("list", 2, Type::list_any())).unwrap();
                let hleaf = r.insert(leaf.clone()).unwrap();
                let built = composite(
                    vec![Port { name: "a".into(), ty: Type::Any }, Port { name: "b".into(), ty: Type::Any }],
                    Type::Any,
                    vec![hlist, hleaf],
                    vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, 1, 0), edge(1, 0, BOUNDARY, 0)],
                );
                let pair = [Some(txt("")), Some(num(2.0))];
                assert_eq!(eval(&built, &pair, &r).into_vec(), eval_eager(&built, &pair, &r), "{name}, given its list");
            }
            assert_eq!(eval(&leaf, &[Some(txt("ab"))], &r).into_vec(), vec![None], "{name} of a text");
            assert_eq!(eval(&over(name, &[9; 32], 0), &[Some(items.clone())], &r).into_vec(), vec![None], "{name} of no function");
        }
        // gt(x, k) = x > k: the input after the list is the function's second.
        let gt = r.insert(Unit { params: vec![Param::frozen("op", ">")], ..prim("compare", 2, Type::Boolean) }).unwrap();
        let xs = list(vec![num(5.0), num(11.0), num(20.0), num(3.0)]);
        for (name, want) in [
            ("filter", Some(list(vec![num(11.0), num(20.0)]))),
            ("find", Some(num(11.0))),
            ("find_index", Some(num(1.0))),
            ("find_last", Some(num(20.0))),
            ("find_last_index", Some(num(2.0))),
            ("any", Some(Value::Boolean(true))),
            ("all", Some(Value::Boolean(false))),
        ] {
            let leaf = over(name, &gt, 1);
            assert_eq!(eval(&leaf, &[Some(xs.clone()), Some(num(10.0))], &r).into_vec(), vec![want.clone()], "{name}");
            assert_eq!(eval_eager(&leaf, &[Some(xs.clone()), Some(num(10.0))], &r), vec![want], "{name}, eagerly");
            assert_eq!(eval(&leaf, &[Some(xs.clone()), None], &r).into_vec(), vec![None], "{name}, its extra absent");
        }
        // An answer that is nothing (a text compared) makes the leaf give nothing.
        let texts = list(vec![txt("a")]);
        assert_eq!(eval(&over("filter", &gt, 1), &[Some(texts.clone()), Some(num(1.0))], &r).into_vec(), vec![None]);
        assert_eq!(eval(&over("any", &gt, 1), &[Some(texts), Some(num(1.0))], &r).into_vec(), vec![None]);
    }

    /// A leaf that stops early applies nothing after the item it stops at. Each application asks
    /// the host for its item (the function is a collection read), so the observer sees every item
    /// applied, at its path. `filter` applies them all. A waiting or absent answer stops a stopping
    /// leaf there too, waiting or giving nothing; `filter` waits rather than give nothing when both
    /// happen, as `map` does.
    #[test]
    fn a_leaf_that_stops_applies_nothing_after_the_item_it_stops_at() {
        struct Asked(Vec<(Vec<u32>, String)>);
        impl Observer for Asked {
            fn answer(&mut self, path: &[u32], request: &Value) -> Answer {
                let Value::Record(r) = request else { return Answer::Absent };
                let Some(Value::Text(k)) = r.get("get") else { return Answer::Absent };
                self.0.push((path.to_vec(), k.clone()));
                match k.as_str() {
                    "no" => Answer::Value(num(0.0)),
                    "yes" => Answer::Value(txt("y")),
                    "wait" => Answer::NotYet,
                    _ => Answer::Absent,
                }
            }
        }
        let mut r = MapResolver::new();
        let get = r.insert(Unit { params: vec![Param::frozen("collection", "answers")], ..prim("collection_get", 1, Type::Any) }).unwrap();
        let run = |name: &str, keys: &[&str]| {
            let xs = list(keys.iter().map(|k| txt(k)).collect());
            let mut asked = Asked(Vec::new());
            let out = eval_at(&over(name, &get, 0), &[Some(&xs)], &r, &mut asked, &mut Vec::new());
            (out.into_vec(), asked.0)
        };
        let asked = |items: &[(u32, &str)]| items.iter().map(|(k, key)| (vec![*k], key.to_string())).collect::<Vec<_>>();
        let value = |v: Value| vec![Slot::Value(v)];
        assert_eq!(run("any", &["no", "yes", "no"]), (value(Value::Boolean(true)), asked(&[(0, "no"), (1, "yes")])));
        assert_eq!(run("all", &["no", "yes"]), (value(Value::Boolean(false)), asked(&[(0, "no")])));
        assert_eq!(run("find", &["no", "yes", "yes"]), (value(txt("yes")), asked(&[(0, "no"), (1, "yes")])));
        assert_eq!(run("find_index", &["no", "yes", "yes"]), (value(num(1.0)), asked(&[(0, "no"), (1, "yes")])));
        assert_eq!(run("find_last", &["yes", "yes", "no"]), (value(txt("yes")), asked(&[(2, "no"), (1, "yes")])));
        assert_eq!(run("find_last_index", &["yes", "yes", "no"]), (value(num(1.0)), asked(&[(2, "no"), (1, "yes")])));
        assert_eq!(run("filter", &["no", "yes", "no", "yes"]), (value(list(vec![txt("yes"), txt("yes")])), asked(&[(0, "no"), (1, "yes"), (2, "no"), (3, "yes")])));
        // What nobody stops at: everything applied.
        assert_eq!(run("any", &["no", "no"]), (value(Value::Boolean(false)), asked(&[(0, "no"), (1, "no")])));
        assert_eq!(run("find_last", &["no", "no"]), (vec![Slot::Absent], asked(&[(1, "no"), (0, "no")])));
        // A waiting or absent answer stops a stopping leaf.
        assert_eq!(run("any", &["wait", "yes"]), (vec![Slot::Pending], asked(&[(0, "wait")])));
        assert_eq!(run("all", &["gone", "no"]), (vec![Slot::Absent], asked(&[(0, "gone")])));
        assert_eq!(run("find_last", &["yes", "gone"]), (vec![Slot::Absent], asked(&[(1, "gone")])));
        // `filter` goes on, as `map` does, so each item's request is asked in the same round.
        assert_eq!(run("filter", &["gone", "wait", "yes"]), (vec![Slot::Pending], asked(&[(0, "gone"), (1, "wait"), (2, "yes")])));
        assert_eq!(run("filter", &["gone", "yes"]), (vec![Slot::Absent], asked(&[(0, "gone"), (1, "yes")])));
    }

    /// `sort_by` orders by the sign of its comparator's answer, keeps equal items in their order,
    /// gives one order for a comparator that isn't consistent, and nothing for an answer that isn't
    /// a number; as the eager oracle does. Each comparison asks at its own path, in order.
    #[test]
    fn sort_by_is_a_stable_merge_sort_on_its_comparators_sign() {
        use crate::eager::eval_eager;
        let mut r = MapResolver::new();
        let sub = r.insert(prim("subtract", 2, Type::Number)).unwrap();
        // by_k(a, b) = a.k - b.k
        let field = r.insert(Unit { params: vec![Param::frozen("name", "k")], ..prim("field", 1, Type::Any) }).unwrap();
        let by_k = r
            .insert(composite(
                vec![Port { name: "a".into(), ty: Type::Any }, Port { name: "b".into(), ty: Type::Any }],
                Type::Number,
                vec![field, field, sub],
                vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 1, 0), edge(0, 0, 2, 0), edge(1, 0, 2, 1), edge(2, 0, BOUNDARY, 0)],
            ))
            .unwrap();
        let constant = |r: &mut MapResolver, ty: &str, v: &str| r.insert(Unit { params: vec![Param::frozen("type", ty), Param::frozen("value", v)], ..prim("const", 0, Type::Any) }).unwrap();
        let (one, minus_zero, word) = (constant(&mut r, "Number", "1"), constant(&mut r, "Number", "-0"), constant(&mut r, "Text", "x"));
        let rec = |k: f64, n: &str| Value::Record(BTreeMap::from([("k".to_string(), num(k)), ("n".to_string(), txt(n))]));
        let nums = |xs: &[f64]| list(xs.iter().map(|x| num(*x)).collect());
        let cases = [
            (sub, nums(&[3.0, 1.0, 2.0]), Some(nums(&[1.0, 2.0, 3.0]))),
            (sub, nums(&[]), Some(nums(&[]))),
            (sub, nums(&[5.0]), Some(nums(&[5.0]))),
            (sub, nums(&[2.0, -1.0, 2.0, 0.5, -1.0, 9.0, 0.0]), Some(nums(&[-1.0, -1.0, 0.0, 0.5, 2.0, 2.0, 9.0]))),
            (by_k, list(vec![rec(2.0, "x"), rec(1.0, "y"), rec(2.0, "z"), rec(1.0, "w")]), Some(list(vec![rec(1.0, "y"), rec(1.0, "w"), rec(2.0, "x"), rec(2.0, "z")]))),
            // 0 and -0 keep the order.
            (minus_zero, nums(&[3.0, 1.0, 2.0]), Some(nums(&[3.0, 1.0, 2.0]))),
            (sub, list(vec![txt("b"), txt("a")]), None),
            (word, nums(&[2.0, 1.0]), None),
        ];
        for (f, xs, want) in cases {
            let leaf = over("sort_by", &f, 0);
            assert_eq!(eval(&leaf, &[Some(xs.clone())], &r).into_vec(), vec![want.clone()], "{xs:?}");
            assert_eq!(eval_eager(&leaf, &[Some(xs.clone())], &r), vec![want], "{xs:?}, eagerly");
        }
        // A comparator that always answers 1 still gives one order, the oracle's, each item once.
        let xs = nums(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let leaf = over("sort_by", &one, 0);
        let first = eval(&leaf, &[Some(xs.clone())], &r).into_vec();
        assert_eq!(first, eval_eager(&leaf, &[Some(xs.clone())], &r));
        let Some(Some(Value::List(got))) = first.first() else { panic!("{first:?}") };
        let mut got: Vec<u64> = got.iter().map(|v| if let Value::Number(n) = v { *n as u64 } else { 0 }).collect();
        got.sort();
        assert_eq!(got, [1, 2, 3, 4, 5]);
        // Each comparison asks the host at the node's path then its place in the sequence.
        struct Paths(Vec<Vec<u32>>);
        impl Observer for Paths {
            fn answer(&mut self, path: &[u32], _: &Value) -> Answer {
                self.0.push(path.to_vec());
                Answer::Value(num(-1.0))
            }
        }
        // asks(a, b) = c[a], a read for each comparison.
        let get = r.insert(Unit { params: vec![Param::frozen("collection", "c")], ..prim("collection_get", 1, Type::Any) }).unwrap();
        let asks = r
            .insert(composite(vec![Port { name: "a".into(), ty: Type::Any }, Port { name: "b".into(), ty: Type::Any }], Type::Any, vec![get], vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, BOUNDARY, 0)]))
            .unwrap();
        let mut paths = Paths(Vec::new());
        let texts = list(vec![txt("a"), txt("b"), txt("c")]);
        let out = eval_at(&over("sort_by", &asks, 0), &[Some(&texts)], &r, &mut paths, &mut Vec::new());
        assert_eq!(out.into_vec(), vec![Slot::Value(texts)]);
        assert_eq!(paths.0, vec![vec![0, 0], vec![1, 0], vec![2, 0]], "[a] with [b], then [a b] with [c]: three comparisons, each read at child 0");
    }

    /// `group_by` puts each item in the list of the text its function answers, the groups in key
    /// order and each in the items' order, hands the function the inputs after the list, and gives
    /// nothing for a key that isn't a text; as the eager oracle does, on its list lent and given.
    /// Every item is applied, and waiting wins over nothing, as in `filter`.
    #[test]
    fn group_by_puts_each_item_in_the_list_of_its_key() {
        use crate::eager::eval_eager;
        let mut r = MapResolver::new();
        let field = r.insert(Unit { params: vec![Param::frozen("name", "g")], ..prim("field", 1, Type::Any) }).unwrap();
        let rec = |g: Value, v: f64| Value::Record(BTreeMap::from([("g".to_string(), g), ("v".to_string(), num(v))]));
        let items = list(vec![rec(txt("x"), 1.0), rec(txt("y"), 2.0), rec(txt("x"), 3.0), rec(txt("10"), 4.0), rec(txt("2"), 5.0)]);
        let want = Value::Record(BTreeMap::from([
            ("10".to_string(), list(vec![rec(txt("10"), 4.0)])),
            ("2".to_string(), list(vec![rec(txt("2"), 5.0)])),
            ("x".to_string(), list(vec![rec(txt("x"), 1.0), rec(txt("x"), 3.0)])),
            ("y".to_string(), list(vec![rec(txt("y"), 2.0)])),
        ]));
        let leaf = over("group_by", &field, 0);
        let cases = [
            (items.clone(), Some(want)),
            (list(vec![]), Some(Value::Record(BTreeMap::new()))),
            (list(vec![rec(txt("x"), 1.0)]), Some(Value::Record(BTreeMap::from([("x".to_string(), list(vec![rec(txt("x"), 1.0)]))])))),
            // An item without the key, and one whose key is a number: nothing, both.
            (list(vec![rec(txt("x"), 1.0), Value::Record(BTreeMap::from([("v".to_string(), num(2.0))]))]), None),
            (list(vec![rec(num(1.0), 1.0)]), None),
            (txt("ab"), None),
        ];
        for (xs, want) in cases {
            assert_eq!(eval(&leaf, &[Some(xs.clone())], &r).into_vec(), vec![want.clone()], "{xs:?}");
            assert_eq!(eval_eager(&leaf, &[Some(xs.clone())], &r), vec![want], "{xs:?}, eagerly");
        }
        // Inside a composite, the list is built there and given to the leaf.
        let hlist = r.insert(prim("list", 2, Type::list_any())).unwrap();
        let hleaf = r.insert(leaf.clone()).unwrap();
        let built = composite(
            vec![Port { name: "a".into(), ty: Type::Any }, Port { name: "b".into(), ty: Type::Any }],
            Type::Any,
            vec![hlist, hleaf],
            vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, 1, 0), edge(1, 0, BOUNDARY, 0)],
        );
        let pair = [Some(rec(txt("b"), 1.0)), Some(rec(txt("a"), 2.0))];
        assert_eq!(eval(&built, &pair, &r).into_vec(), eval_eager(&built, &pair, &r));
        // by(x, k) = x[k]: the input after the list is the function's second.
        let lookup = r.insert(prim("lookup", 2, Type::Any)).unwrap();
        let by = over("group_by", &lookup, 1);
        let xs = list(vec![Value::Record(BTreeMap::from([("k".to_string(), txt("p"))])), Value::Record(BTreeMap::from([("k".to_string(), txt("q"))]))]);
        let want = Value::Record(BTreeMap::from([
            ("p".to_string(), list(vec![Value::Record(BTreeMap::from([("k".to_string(), txt("p"))]))])),
            ("q".to_string(), list(vec![Value::Record(BTreeMap::from([("k".to_string(), txt("q"))]))])),
        ]));
        assert_eq!(eval(&by, &[Some(xs.clone()), Some(txt("k"))], &r).into_vec(), vec![Some(want.clone())]);
        assert_eq!(eval_eager(&by, &[Some(xs.clone()), Some(txt("k"))], &r), vec![Some(want)]);
        assert_eq!(eval(&by, &[Some(xs), None], &r).into_vec(), vec![None], "its extra absent");
        assert_eq!(eval(&over("group_by", &[9; 32], 0), &[Some(items)], &r).into_vec(), vec![None], "no function");
        // Each item asks the host for its key: all of them, waiting winning over nothing.
        struct Keys(Vec<Vec<u32>>);
        impl Observer for Keys {
            fn answer(&mut self, path: &[u32], request: &Value) -> Answer {
                self.0.push(path.to_vec());
                let Value::Record(r) = request else { return Answer::Absent };
                match r.get("get") {
                    Some(Value::Text(k)) if k == "wait" => Answer::NotYet,
                    Some(Value::Text(k)) if k == "gone" => Answer::Absent,
                    Some(Value::Text(k)) => Answer::Value(txt(&k.to_uppercase())),
                    _ => Answer::Absent,
                }
            }
        }
        let get = r.insert(Unit { params: vec![Param::frozen("collection", "keys")], ..prim("collection_get", 1, Type::Any) }).unwrap();
        let run = |keys: &[&str]| {
            let xs = list(keys.iter().map(|k| txt(k)).collect());
            let mut asked = Keys(Vec::new());
            let out = eval_at(&over("group_by", &get, 0), &[Some(&xs)], &r, &mut asked, &mut Vec::new());
            (out.into_vec(), asked.0)
        };
        let grouped = Value::Record(BTreeMap::from([("A".to_string(), list(vec![txt("a"), txt("a")])), ("B".to_string(), list(vec![txt("b")]))]));
        assert_eq!(run(&["a", "b", "a"]), (vec![Slot::Value(grouped)], vec![vec![0], vec![1], vec![2]]));
        assert_eq!(run(&["gone", "wait", "a"]), (vec![Slot::Pending], vec![vec![0], vec![1], vec![2]]));
        assert_eq!(run(&["gone", "a"]), (vec![Slot::Absent], vec![vec![0], vec![1]]));
    }

    /// A JSON leaf of `fn_hash`, its function's extras after the first input, and its params.
    fn json_leaf(name: &str, fn_hash: &Hash, extras: usize, more: &[(&str, &str)]) -> Unit {
        let first = if name == "json_parse" { "text" } else { "value" };
        let mut inputs = vec![Port { name: first.into(), ty: Type::Any }];
        inputs.extend((0..extras).map(|i| Port { name: format!("extra{i}"), ty: Type::Any }));
        let mut params = vec![Param::frozen("fn", hex(fn_hash))];
        params.extend(more.iter().map(|&(k, v)| Param::frozen(k, v)));
        Unit { body: UnitBody::Primitive { name: name.into() }, params, inputs, outputs: vec![Port { name: "out".into(), ty: Type::Any }] }
    }

    /// `json_parse` with a reviver and `json_stringify` with a replacer apply their function to
    /// each key and value, hand it the inputs after the first, leave out what it answers nothing
    /// for, and give what the eager oracle gives; a waiting answer stops either, waiting.
    #[test]
    fn a_reviver_and_a_replacer_apply_their_function_to_each_key_and_value() {
        use crate::eager::eval_eager;
        let mut r = MapResolver::new();
        // tens(k, v, by) = v * by: a number times the extra, and nothing for anything else.
        let mul = r.insert(prim("multiply", 2, Type::Number)).unwrap();
        let tens = r
            .insert(composite(
                vec![Port { name: "k".into(), ty: Type::Any }, Port { name: "v".into(), ty: Type::Any }, Port { name: "by".into(), ty: Type::Any }],
                Type::Any,
                vec![mul],
                vec![edge(BOUNDARY, 1, 0, 0), edge(BOUNDARY, 2, 0, 1), edge(0, 0, BOUNDARY, 0)],
            ))
            .unwrap();
        // value_of(k, v) = v: every value as it is.
        let same = r.insert(Unit { params: vec![Param::frozen("type", "Number"), Param::frozen("value", "0")], ..prim("default", 1, Type::Any) }).unwrap();
        let value_of = r
            .insert(composite(
                vec![Port { name: "k".into(), ty: Type::Any }, Port { name: "v".into(), ty: Type::Any }],
                Type::Any,
                vec![same],
                vec![edge(BOUNDARY, 1, 0, 0), edge(0, 0, BOUNDARY, 0)],
            ))
            .unwrap();
        let text = |s: &str| Some(txt(s));
        let cases: Vec<(Unit, Vec<Option<Value>>, Option<Value>)> = vec![
            // A reviver that answers numbers only: every record and list answered nothing, so the whole is nothing.
            (json_leaf("json_parse", &tens, 1, &[]), vec![text("[1, 2]"), Some(num(10.0))], None),
            (json_leaf("json_parse", &tens, 1, &[]), vec![text("3"), Some(num(10.0))], Some(num(30.0))),
            (json_leaf("json_parse", &tens, 1, &[]), vec![text("3"), None], None),
            (json_leaf("json_parse", &value_of, 0, &[]), vec![text(r#"{"a": [1, {"b": null}]}"#)], crate::json::parse(r#"{"a": [1, {"b": null}]}"#)),
            (json_leaf("json_parse", &value_of, 0, &[]), vec![text("{")], None),
            (json_leaf("json_stringify", &value_of, 0, &[]), vec![crate::json::parse(r#"{"a": [1, {"b": null}]}"#)], text(r#"{"a":[1,{"b":null}]}"#)),
            (json_leaf("json_stringify", &value_of, 0, &[("indent", " ")]), vec![crate::json::parse(r#"{"a": [1]}"#)], text("{\n \"a\": [\n  1\n ]\n}")),
            (json_leaf("json_stringify", &tens, 1, &[]), vec![Some(num(4.0)), Some(num(10.0))], text("40")),
            (json_leaf("json_stringify", &tens, 1, &[]), vec![crate::json::parse("[4]"), Some(num(10.0))], None),
            (json_leaf("json_stringify", &value_of, 0, &[("keys", r#"["a"]"#)]), vec![crate::json::parse(r#"{"a": 1}"#)], None),
        ];
        for (unit, inputs, want) in cases {
            assert_eq!(eval(&unit, &inputs, &r).into_vec(), vec![want.clone()], "{unit:?} on {inputs:?}");
            assert_eq!(eval_eager(&unit, &inputs, &r), vec![want], "{unit:?} on {inputs:?}, eagerly");
        }
        // Without a function, the leaf as it is: `fn` unset.
        let plain = Unit { params: vec![Param::frozen("fn", "")], ..json_leaf("json_parse", &tens, 0, &[]) };
        assert_eq!(eval(&plain, &[text("[1]")], &r).into_vec(), vec![crate::json::parse("[1]")]);
        // Each application asks the host at the node's path then its place in the walk; a
        // waiting answer stops the walk there, waiting.
        struct Asked(Vec<(Vec<u32>, String)>);
        impl Observer for Asked {
            fn answer(&mut self, path: &[u32], request: &Value) -> Answer {
                let Value::Record(r) = request else { return Answer::Absent };
                let Some(Value::Text(k)) = r.get("get") else { return Answer::Absent };
                self.0.push((path.to_vec(), k.clone()));
                if k == "wait" { Answer::NotYet } else { Answer::Value(num(1.0)) }
            }
        }
        let get = r.insert(Unit { params: vec![Param::frozen("collection", "c")], ..prim("collection_get", 1, Type::Any) }).unwrap();
        let by_key = r
            .insert(composite(vec![Port { name: "k".into(), ty: Type::Any }, Port { name: "v".into(), ty: Type::Any }], Type::Any, vec![get], vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, BOUNDARY, 0)]))
            .unwrap();
        let run = |name: &str, input: Value| {
            let mut asked = Asked(Vec::new());
            let out = eval_at(&json_leaf(name, &by_key, 0, &[]), &[Some(&input)], &r, &mut asked, &mut Vec::new());
            (out.into_vec(), asked.0)
        };
        let at = |items: &[(u32, &str)]| items.iter().map(|(k, key)| (vec![*k, 0], key.to_string())).collect::<Vec<_>>();
        assert_eq!(run("json_parse", txt(r#"{"a": 1, "b": [2]}"#)), (vec![Slot::Value(num(1.0))], at(&[(0, "a"), (1, "0"), (2, "b"), (3, "")])));
        assert_eq!(run("json_parse", txt(r#"{"wait": 1, "z": 2}"#)), (vec![Slot::Pending], at(&[(0, "wait")])));
        assert_eq!(run("json_stringify", crate::json::parse(r#"{"a": 1}"#).unwrap()), (vec![Slot::Value(txt("1"))], at(&[(0, "")])));
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
        // add(acc, true) fails ⇒ the whole fold is None.
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let f = fold_unit(&hadd);
        let xs = list(vec![num(3.0), Value::Boolean(true)]);
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

    /// Which values are given and which copied (`heavy`): a scalar, a short text, and a list or
    /// record of up to 8 of those are copied; anything longer, or holding more, is given.
    #[test]
    fn small_values_are_copied_and_large_ones_given() {
        let rec = |n: usize, v: Value| Value::Record((0..n).map(|k| (format!("f{k}"), v.clone())).collect());
        let nums = |n: usize| list((0..n).map(|k| num(k as f64)).collect());
        for small in [num(1.0), Value::Boolean(true), text(""), text(&"x".repeat(32)), nums(0), nums(8), rec(3, num(1.0)), rec(8, text("ok")), list(vec![Value::Boolean(false), text("a")])] {
            assert!(!heavy(&small), "{small:?} is copied");
        }
        let nested = list(vec![nums(1)]);
        let option = Value::Sum { arm: 0, payload: Box::new(num(1.0)) };
        for large in [text(&"x".repeat(33)), nums(9), rec(9, num(1.0)), rec(1, text(&"x".repeat(33))), nested, rec(1, nums(0)), option] {
            assert!(heavy(&large), "{large:?} is given");
        }
    }

    /// Giving and lending (a value moved to the node that builds on it, a choice reading through
    /// to where its value is kept) change no value. On generated functions over lists, texts and
    /// records, of the leaves that build on what they read, choices, composites, and `map`s and
    /// `fold`s of earlier functions, each called once, applied by a `fold` (its accumulator given),
    /// by a `map` (its items lent) and by `filter`, `find_last` and `sort_by`, `eval` gives what
    /// evaluating every node gives. An observer
    /// that reads what each node produced sees the same steps at the same paths as one that
    /// doesn't, and the same outputs.
    #[test]
    fn giving_and_lending_agree_with_eager_on_generated_graphs() {
        use crate::eager::eval_eager;
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
        /// Every step, and what each node produced when it reads that.
        struct Seen {
            reads: bool,
            steps: Vec<(u8, Vec<u32>)>,
            produced: Vec<Vec<Slot>>,
        }
        impl Observer for Seen {
            fn enter(&mut self, path: &[u32]) {
                self.steps.push((b'e', path.to_vec()));
            }
            fn leave(&mut self, path: &[u32]) {
                self.steps.push((b'l', path.to_vec()));
            }
            fn produced(&mut self, path: &[u32], outputs: &[Slot]) {
                self.steps.push((b'p', path.to_vec()));
                if self.reads {
                    self.produced.push(outputs.to_vec());
                }
            }
            fn reads_produced(&self) -> bool {
                self.reads
            }
        }
        const LEAVES: &[(&str, &[(&str, &str)])] = &[
            ("guard", &[]),
            ("coalesce", &[]),
            ("append", &[]),
            ("extend", &[]),
            ("cons", &[]),
            ("tail", &[]),
            ("head", &[]),
            ("get", &[]),
            ("length", &[]),
            ("concat", &[]),
            ("insert", &[]),
            ("lookup", &[]),
            ("remove", &[]),
            ("with_field", &[("name", "a")]),
            ("field", &[("name", "a")]),
            ("select", &[]),
            ("pair", &[]),
            ("is_truthy", &[]),
            ("equals", &[]),
            ("subtract", &[]),
            ("at", &[]),
            ("reverse", &[]),
            ("sort", &[]),
            ("unique", &[]),
            ("with_item", &[]),
            ("to_spliced", &[]),
            ("slice", &[("mode", "end")]),
            ("flat", &[("depth", "1")]),
            ("fill", &[]),
            ("copy_within", &[]),
            ("entries", &[]),
            ("includes", &[]),
            ("values", &[]),
            ("from_entries", &[]),
            ("merge", &[]),
        ];
        let rec = |kv: &[(&str, Value)]| Value::Record(kv.iter().map(|(k, v)| (k.to_string(), v.clone())).collect());
        let values = [
            None,
            Some(Value::Boolean(true)),
            Some(Value::Boolean(false)),
            Some(num(0.0)),
            Some(num(1.0)),
            Some(text("x")),
            Some(text("a")),
            Some(list(vec![])),
            Some(list(vec![num(1.0), text("a")])),
            Some(list(vec![list(vec![num(2.0)])])),
            Some(rec(&[])),
            Some(rec(&[("a", num(1.0)), ("x", list(vec![]))])),
            // Heavy by size alone (`heavy`): the small ones above are copied, these given.
            Some(text("a text longer than the thirty-two bytes")),
            Some(list((0..9).map(|k| num(k as f64)).collect())),
        ];

        fn graph(rng: &mut Rng, pool: &[(Hash, usize)]) -> Unit {
            let n = 2 + rng.below(9);
            let mut children = Vec::new();
            let mut edges = Vec::new();
            for c in 0..n {
                let (h, k) = pool[rng.below(pool.len())];
                children.push(h);
                for p in 0..k {
                    match rng.below(12) {
                        0 => {}
                        1..=4 => edges.push(edge(BOUNDARY, rng.below(2) as u32, c as u32, p as u32)),
                        _ if c > 0 => edges.push(edge(rng.below(c) as u32, 0, c as u32, p as u32)),
                        _ => edges.push(edge(BOUNDARY, rng.below(2) as u32, c as u32, p as u32)),
                    }
                }
            }
            // One output, or two, the second sometimes an input itself: what a run moves out at
            // its end must still be there for every output that reads it.
            let outs = 1 + rng.below(2);
            edges.push(edge(rng.below(n) as u32, 0, BOUNDARY, 0));
            if outs == 2 {
                match rng.below(3) {
                    0 => edges.push(edge(BOUNDARY, rng.below(2) as u32, BOUNDARY, 1)),
                    _ => edges.push(edge(rng.below(n) as u32, 0, BOUNDARY, 1)),
                }
            }
            multi_composite(vec![Port { name: "a".into(), ty: Type::Any }, Port { name: "b".into(), ty: Type::Any }], vec![Type::Any; outs], children, edges)
        }

        let mut r = MapResolver::new();
        let mut pool: Vec<(Hash, usize)> = LEAVES
            .iter()
            .map(|&(name, params)| {
                let mut unit = crate::interface_of(name).unwrap_or_else(|| panic!("no leaf {name}"));
                unit.params = params.iter().map(|&(k, v)| Param::frozen(k, v)).collect();
                let k = unit.inputs.len();
                (r.insert(unit).unwrap(), k)
            })
            .collect();
        // A `fold` and a `map` of a leaf: each application a leaf given its item.
        for name in ["append", "extend", "cons", "concat", "pair"] {
            let leaf = pool[LEAVES.iter().position(|&(n, _)| n == name).unwrap()].0;
            let mut fold = fold_unit(&leaf);
            fold.inputs[0].ty = Type::Any;
            let mut map = map_unit(&leaf);
            map.inputs.push(Port { name: "extra".into(), ty: Type::Any });
            for unit in [fold, map] {
                pool.push((r.insert(unit).unwrap(), 2));
            }
        }
        // The leaves that test each item, and `sort_by`, of a leaf: the list lent or given to them.
        for (hof, name, extras) in [("filter", "equals", 1), ("find", "equals", 1), ("find_last", "is_truthy", 0), ("any", "is_truthy", 0), ("find_index", "equals", 1), ("sort_by", "subtract", 0), ("group_by", "coalesce", 0)] {
            let leaf = pool[LEAVES.iter().position(|&(n, _)| n == name).unwrap()].0;
            pool.push((r.insert(over(hof, &leaf, extras)).unwrap(), 1 + extras));
        }
        // And a `fold` and a `map` of each of those: a function that is itself a bare `map` or
        // `fold`, handed the outer one's extras at every item.
        let hofs: Vec<Hash> = pool[LEAVES.len()..].iter().map(|&(hof, _)| hof).collect();
        for hof in hofs {
            let mut fold = fold_unit(&hof);
            fold.inputs[0].ty = Type::Any;
            let mut map = map_unit(&hof);
            map.inputs.push(Port { name: "extra".into(), ty: Type::Any });
            for unit in [fold, map] {
                pool.push((r.insert(unit).unwrap(), 2));
            }
        }
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        let mut checked = 0;
        for round in 0..1500 {
            let g = graph(&mut rng, &pool);
            let Ok(h) = r.insert(g.clone()) else { continue };
            let mut fold = fold_unit(&h);
            fold.inputs[0].ty = Type::Any;
            let mut map = map_unit(&h);
            map.inputs.push(Port { name: "extra".into(), ty: Type::Any });
            let (filter, find_last, sort_by) = (over("filter", &h, 1), over("find_last", &h, 1), over("sort_by", &h, 0));
            for _ in 0..3 {
                let pick = |rng: &mut Rng| values[rng.below(values.len())].clone();
                let (a, b) = (pick(&mut rng), pick(&mut rng));
                let items = Some(list((0..rng.below(4)).filter_map(|_| pick(&mut rng)).collect()));
                for (unit, inputs) in [
                    (&g, vec![a.clone(), b.clone()]),
                    (&fold, vec![a.clone(), items.clone()]),
                    (&map, vec![items.clone(), b.clone()]),
                    (&filter, vec![items.clone(), b.clone()]),
                    (&find_last, vec![items.clone(), b.clone()]),
                    (&sort_by, vec![items.clone()]),
                ] {
                    let lazy = eval(unit, &inputs, &r).into_vec();
                    let show = |u: &Unit| -> String {
                        let UnitBody::Composite { children, edges } = &u.body else { return format!("{:?}", u.body) };
                        let names: Vec<String> = children.iter().map(|c| r.resolve(c).map_or("?".into(), |c| format!("{:?}{:?}", c.body, c.params))).collect();
                        format!("{names:#?}\n{edges:?}")
                    };
                    assert_eq!(lazy, eval_eager(unit, &inputs, &r), "{unit:?} on {inputs:?}\nfunction {}", show(&g));
                    let mut reading = Seen { reads: true, steps: Vec::new(), produced: Vec::new() };
                    let mut blind = Seen { reads: false, steps: Vec::new(), produced: Vec::new() };
                    assert_eq!(eval_observed(unit, &inputs, &r, &mut reading).into_vec(), lazy);
                    assert_eq!(eval_observed(unit, &inputs, &r, &mut blind).into_vec(), lazy);
                    assert_eq!(reading.steps, blind.steps, "{unit:?} on {inputs:?}");
                    assert!(reading.produced.iter().all(|outs| !outs.is_empty()), "a reading observer is handed every output");
                    checked += 1;
                }
            }
            // Some functions join the pool, as themselves (a composite inside a composite) and
            // mapped and folded, so later functions nest them and give them what they build on.
            if round % 50 == 0 && pool.len() < LEAVES.len() + 54 {
                pool.push((h, 2));
                let mut fold = fold_unit(&h);
                fold.inputs[0].ty = Type::Any;
                if let Ok(hf) = r.insert(fold) {
                    pool.push((hf, 2));
                }
                let mut map = map_unit(&h);
                map.inputs.push(Port { name: "extra".into(), ty: Type::Any });
                if let Ok(hm) = r.insert(map) {
                    pool.push((hm, 2));
                }
            }
        }
        assert!(checked > 12_000, "{checked}");
    }

    /// A `map` whose function is a bare `fold` hands the fold its extra as the list at every item:
    /// given its own list (here as an outer fold's accumulator), it still lends the extra, so the
    /// second item's fold has its list too.
    #[test]
    fn a_map_of_a_bare_fold_keeps_its_extra_for_every_item() {
        use crate::eager::eval_eager;
        let mut r = MapResolver::new();
        let happend = r.insert(prim("append", 2, Type::Any)).unwrap();
        let mut inner = fold_unit(&happend);
        inner.inputs[0].ty = Type::Any;
        let hinner = r.insert(inner).unwrap();
        let mut m = map_unit(&hinner);
        m.inputs.push(Port { name: "extra".into(), ty: Type::Any });
        let hm = r.insert(m).unwrap();
        let want = vec![Some(list(vec![list(vec![num(0.0), num(1.0), num(2.0)]), list(vec![num(10.0), num(1.0), num(2.0)])]))];
        let inputs = vec![Some(list(vec![list(vec![num(0.0)]), list(vec![num(10.0)])])), Some(list(vec![list(vec![num(1.0), num(2.0)])]))];

        // fold(map(fn: fold(append))): the map is given the outer fold's accumulator.
        let outer = fold_unit(&hm);
        assert_eq!(eval_eager(&outer, &inputs, &r), want);
        assert_eq!(eval(&outer, &inputs, &r).into_vec(), want);

        // The same through a composite step(acc, x) = map(acc, x, fn: fold(append)), which gives
        // its accumulator to its map child.
        let step = composite(
            vec![Port { name: "acc".into(), ty: Type::Any }, Port { name: "x".into(), ty: Type::Any }],
            Type::Any,
            vec![hm],
            vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, BOUNDARY, 0)],
        );
        let hstep = r.insert(step).unwrap();
        let mut outer = fold_unit(&hstep);
        outer.inputs[0].ty = Type::Any;
        assert_eq!(eval_eager(&outer, &inputs, &r), want);
        assert_eq!(eval(&outer, &inputs, &r).into_vec(), want);
    }

    /// A fold that wraps its accumulator a level each step (`range(n).reduce((acc) => [acc], 0)`)
    /// builds to exactly the bound and no further: 256 steps nest 256 deep, the 257th gives
    /// nothing, and 10,000 give nothing at once on a thread a host's size, where they once nested
    /// 10,000 deep and overflowed whatever walked the value next. A `map` whose results are as deep
    /// as a value may nest, a `group_by` of a list that deep, and a reviver whose answers take the
    /// parsed value past it give nothing as well; the eager oracle agrees on each.
    #[test]
    fn a_value_built_a_level_at_a_time_stops_at_the_bound() {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let mut r = MapResolver::new();
                let one = r.insert(prim("list", 1, Type::list_any())).unwrap();
                let wrap = r.insert(composite(vec![nport("acc"), nport("item")], Type::Any, vec![one], vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, BOUNDARY, 0)])).unwrap();
                let fold = fold_unit(&wrap);
                let range = |n: usize| Value::List((0..n).map(|i| num(i as f64)).collect());
                let nested = |depth: usize| (0..depth).fold(num(0.0), |v, _| Value::List(vec![v]));
                let folded = |n: usize| {
                    let inputs = [Some(num(0.0)), Some(range(n))];
                    let lazy = eval(&fold, &inputs, &r).into_vec();
                    assert_eq!(lazy, crate::eager::eval_eager(&fold, &inputs, &r), "eager agrees at {n}");
                    lazy.into_iter().next().flatten()
                };
                assert_eq!(folded(MAX_VALUE_DEPTH), Some(nested(MAX_VALUE_DEPTH)));
                assert_eq!(folded(MAX_VALUE_DEPTH + 1), None);
                assert_eq!(folded(10_000), None);

                // map's results go a level down: as deep as a value may nest, nothing.
                let map = map_unit(&one);
                let items = |depth: usize| [Some(Value::List(vec![nested(depth)]))];
                for (depth, builds) in [(MAX_VALUE_DEPTH - 2, true), (MAX_VALUE_DEPTH - 1, false)] {
                    let lazy = eval(&map, &items(depth), &r).into_vec();
                    assert_eq!(lazy, crate::eager::eval_eager(&map, &items(depth), &r));
                    assert_eq!(lazy[0].is_some(), builds, "map over items {depth} deep");
                }

                // group_by's items go a level down, into their group's list.
                let key = r.insert(prim("type_of", 1, Type::Text)).unwrap();
                let group = over("group_by", &key, 0);
                for (depth, builds) in [(MAX_VALUE_DEPTH - 1, true), (MAX_VALUE_DEPTH, false)] {
                    let inputs = [Some(nested(depth))];
                    let lazy = eval(&group, &inputs, &r).into_vec();
                    assert_eq!(lazy, crate::eager::eval_eager(&group, &inputs, &r));
                    assert_eq!(lazy[0].is_some(), builds, "group_by of a list {depth} deep");
                }

                // A reviver answering `[value, extra]` puts `extra` three levels down: under its
                // pair, under the root's member, under the root's own pair.
                let pair = r.insert(prim("pair", 2, Type::list_any())).unwrap();
                let reviver = r
                    .insert(composite(vec![nport("key"), nport("value"), nport("extra")], Type::Any, vec![pair], vec![edge(BOUNDARY, 1, 0, 0), edge(BOUNDARY, 2, 0, 1), edge(0, 0, BOUNDARY, 0)]))
                    .unwrap();
                let parse = json_leaf("json_parse", &reviver, 1, &[]);
                for (depth, builds) in [(MAX_VALUE_DEPTH - 3, true), (MAX_VALUE_DEPTH - 2, false)] {
                    let inputs = [Some(Value::Text("{\"a\": 1}".into())), Some(nested(depth))];
                    let lazy = eval(&parse, &inputs, &r).into_vec();
                    assert_eq!(lazy, crate::eager::eval_eager(&parse, &inputs, &r));
                    assert_eq!(lazy[0].is_some(), builds, "a reviver's answers {depth} deep");
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
