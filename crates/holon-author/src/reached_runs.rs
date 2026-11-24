//! Where each doing site of a text is reached, read off a run, beside what the run did there
//! (step C1 of the one rule for reached, `lang::reached`). A handler runs in-process, as the golden
//! record runs it (golden.rs); each site's path is read on the values the run computed, and what
//! today's graph did at the site (an effect made, a request or a read asked) is read off the same
//! run. A test of the path the run never computed is "not known". Used by tests alone: nothing a
//! flow does reads it.

use std::collections::{BTreeMap, HashMap, HashSet};

use holon_core::{eval_observed, hex, parse_hash, Answer, MapResolver, Observer, Resolver, Slot, Unit, UnitBody, Value};

use crate::composites::Names;
use crate::lang::ast::{Expr, ExprKind, Pos, Span};
use crate::lang::lower::Found;
use crate::lang::reached::{holds, Atom, Seen, Site};
use crate::lang::{ts, Lowered};

/// What one run computed and asked, by path.
#[derive(Default)]
pub struct Run {
    pub produced: HashMap<Vec<u32>, Vec<Slot>>,
    pub entered: HashSet<Vec<u32>>,
    pub asked: Vec<Vec<u32>>,
    answer: Option<fn(&Value) -> Answer>,
}

impl Observer for Run {
    fn enter(&mut self, path: &[u32]) {
        self.entered.insert(path.to_vec());
    }
    fn produced(&mut self, path: &[u32], outputs: &[Slot]) {
        self.produced.insert(path.to_vec(), outputs.to_vec());
    }
    fn answer(&mut self, path: &[u32], request: &Value) -> Answer {
        self.asked.push(path.to_vec());
        self.answer.map_or(Answer::Absent, |a| a(request))
    }
}

/// A text compiled with its sites, each site's test parts and what each was lowered to.
pub struct Watched {
    pub lowered: Lowered,
    pub parts: Vec<Expr>,
    pub found: Found,
}

impl Watched {
    pub fn of(src: &str) -> Result<Watched, String> {
        let (lowered, parts, found) = ts::compile_watched(src).map_err(|e| e.to_string())?;
        Ok(Watched { lowered, parts, found })
    }

    /// The nodes a part was lowered to; a binding's name read nowhere, its own node.
    fn nodes(&self, part: &Expr) -> Vec<(String, usize)> {
        let i = self.parts.iter().position(|p| p == part).expect("a part of a test");
        if !self.found.nodes[i].is_empty() {
            return self.found.nodes[i].clone();
        }
        match &part.kind {
            ExprKind::Name(n) => self.lowered.names.iter().filter(|(_, b)| *b == n).map(|(id, _)| (id.clone(), 0)).collect(),
            _ => vec![],
        }
    }
}

/// Whether `inner` is written within `outer`.
pub fn within(inner: Span, outer: Span) -> bool {
    let at = |p: Pos| (p.line, p.col);
    at(outer.start) <= at(inner.start) && at(inner.end) <= at(outer.end)
}

/// What a site's path gives on one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reached {
    Yes,
    No,
    /// A test it needed (the atom at this index) the run never computed.
    NotKnown(usize),
}

/// A composite the run went into: its path, and each of its children's node id, in order.
#[derive(Clone)]
struct Context {
    path: Vec<u32>,
    children: Vec<String>,
    /// The composite and the context it is a child of: where a function's call is.
    parent: Option<(Box<Context>, u32)>,
    /// The function whose body it is.
    function: Option<String>,
}

impl Context {
    fn child(&self, id: &str) -> Option<u32> {
        self.children.iter().position(|c| c == id).map(|k| k as u32)
    }
}

/// One site on one run, in one place it is run (a handler's body once, a function's body once a
/// call): what its path gives, and what today's graph did there.
#[derive(Clone, Debug)]
pub struct SiteRun {
    /// The site, by its place among the text's.
    pub site: usize,
    pub handler: String,
    pub input: usize,
    /// Where it was run: the handler's or the function call's path.
    pub at: Vec<u32>,
    pub reached: Reached,
    /// What today's graph did at the site's own step: made it (an effect, a request or read
    /// asked), or not; `None` where the site has no step of its own (a state key written in
    /// several places is one step, at its last write).
    pub today: Option<bool>,
    /// Where the site is in a `try` block with a catch: whether the block's test held.
    pub kept: Option<bool>,
    /// Whether what it needs ([`Site::needs`]) has values: `None` where one was never computed.
    /// Where one has none, Holon's own rule doesn't run it.
    pub has_values: Option<bool>,
    /// Whether, outside a `try`, a statement before it gave nothing at a step that throws in
    /// JavaScript, which would have stopped there; the flow goes on (labelled by [`PAST_NOTHING`]).
    pub past_nothing: bool,
    /// Whether its key is written at another site of the same body too: the graph writes a key
    /// once per run, at one step for all of them, so which site's value it wrote is its own column.
    pub one_key: bool,
}

/// How a site's run stands beside what today's graph did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    /// Made where reached and given what it needs, and not made elsewhere.
    Agrees,
    /// Reached, a value it needs has none, and not made: Holon's own rule.
    NothingToGoOn,
    /// Made, where its path says it isn't reached.
    MadeUnreached,
    /// Not made, where its path says it is reached and it has what it needs.
    MissedReached,
    /// Made, where a test of its path was never computed by the run: the step went out without
    /// what reaches it being looked at, where JavaScript looks at it first.
    MadeUnlooked,
    /// A key written at several sites, by one step: compared on its own column.
    OneKey,
    /// Reached, in a `try` block that failed, and not made: the try's promise, that a block that
    /// fails keeps nothing (its requests aside), not a difference.
    NotKept,
    /// A test its path or a value it needs was never computed.
    NotKnown,
    /// It has no step of its own (a state key written in several places is one step).
    NoStepOfItsOwn,
    /// Made past a line outside a `try` that gave nothing where JavaScript throws: labelled by
    /// [`PAST_NOTHING`].
    PastNothing(PastNothing),
    /// What it does isn't compared: a function entered (its body's sites are), a throw (its
    /// answer is the route's), a read of state (supplied as the run begins, never asked).
    NotCompared,
}

impl SiteRun {
    pub fn verdict(&self, site: &Site) -> Verdict {
        if matches!(site.does, "enters" | "throws" | "reads state") {
            return Verdict::NotCompared;
        }
        if self.one_key {
            return Verdict::OneKey;
        }
        let Some(made) = self.today else { return Verdict::NoStepOfItsOwn };
        if !made && self.reached == Reached::Yes && self.kept == Some(false) {
            return Verdict::NotKept;
        }
        if made && self.past_nothing {
            return Verdict::PastNothing(PAST_NOTHING);
        }
        match (self.reached, self.has_values, made) {
            (Reached::NotKnown(_), _, true) => Verdict::MadeUnlooked,
            (Reached::NotKnown(_), ..) => Verdict::NotKnown,
            (Reached::No, _, false) => Verdict::Agrees,
            (Reached::No, _, true) => Verdict::MadeUnreached,
            (Reached::Yes, Some(false), false) => Verdict::NothingToGoOn,
            (Reached::Yes, _, true) => Verdict::Agrees,
            (Reached::Yes, Some(true), false) => Verdict::MissedReached,
            (Reached::Yes, None, false) => Verdict::NotKnown,
        }
    }
}

/// What a run past a line that gave nothing, outside a `try`, is: a difference owed to
/// JavaScript, or Holon's own rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PastNothing {
    /// JavaScript throws at the line and runs nothing below it; the flow owes that.
    Owed,
    /// Holon's own rule: a line that gives nothing is a value missing, not a failure.
    OwnRule,
}

/// The one switch for it. The owner's question is open (2026-11-23), so it is owed for now;
/// either answer is this line.
pub const PAST_NOTHING: PastNothing = PastNothing::Owed;

/// The steps that throw in JavaScript where they give nothing (each by its leaf): a text that
/// isn't JSON, a request or another flow's action that failed. (A `throw` is not among them: a
/// flow stops there too, as a return of nothing, §6.5a.) Outside a `try` the flow goes on past them (`lang::reached`); a run past one is labelled by [`PAST_NOTHING`].
pub const THROWS_IN_JAVASCRIPT: &[&str] = &["json_parse", "http_request", "http_upload", "http_stream", "flow_call"];

/// The doing steps a site's own step is among: what makes an effect, asks a request or a read,
/// or waits.
fn does_something(run_paths: &Run, path: &[u32]) -> bool {
    let effect = run_paths.produced.get(path).is_some_and(|outs| outs.iter().any(|o| matches!(o, Slot::Value(Value::Effect { .. }))));
    effect || run_paths.asked.iter().any(|a| a.starts_with(path))
}

/// Each handler of a text's server: its label, its unit's hash, and the node each child of its
/// handler composite is.
pub struct Handler {
    pub label: String,
    pub kind: &'static str,
    pub method: String,
    /// A route's or a websocket's path; a file handler's folder.
    pub path: String,
    pub hash: String,
    pub children: Vec<String>,
}

/// The handlers of `w`'s server, their units in `store`.
pub fn handlers(w: &Watched, store: &mut MapResolver) -> Vec<Handler> {
    let doc = &w.lowered.doc;
    crate::text::restore(doc, store, &Names::new());
    let port = doc.server.as_ref().map(|x| x.port.clone()).unwrap_or_default();
    let server = crate::flow::derive_server(&doc.nodes, &doc.edges, store, &port, doc.unit_settings());
    let trace = |label: &str| server.trace.get(label).cloned().unwrap_or_default();
    let mut out = Vec::new();
    let mut add = |label: String, kind, method: &str, path: &str, hash: &str| {
        out.push(Handler { children: trace(&label), label, kind, method: method.into(), path: path.into(), hash: hash.into() });
    };
    for e in &server.endpoints {
        add(format!("{} {}", e.method, e.path), "route", &e.method, &e.path, &e.hash);
    }
    for a in &server.actions {
        add(a.label.clone(), "action", "", "", &a.hash);
    }
    for c in &server.clocks {
        add(c.label.clone(), "clock", "", "", &c.hash);
    }
    for f in &server.files {
        add(f.label.clone(), "file", "", &f.folder, &f.hash);
    }
    for m in &server.mails {
        add(m.label.clone(), "mail", "", "", &m.hash);
    }
    for f in &server.feeds {
        add(f.label.clone(), "feed", "", "", &f.hash);
    }
    for q in &server.queues {
        add(q.label.clone(), "queue", "", "", &q.hash);
    }
    for s in &server.websockets {
        add(s.label.clone(), "websocket", "", &s.path, &s.hash);
    }
    out
}

/// What a test's part is in a context: the first of its nodes the context holds, as the run left
/// it; "not known" where none was computed.
fn seen(w: &Watched, ctx: &Context, run: &Run, part: &Expr) -> Seen {
    for (id, port) in w.nodes(part) {
        let Some(k) = ctx.child(&id) else { continue };
        let mut at = ctx.path.clone();
        at.push(k);
        if let Some(outs) = run.produced.get(&at) {
            return match outs.get(port) {
                Some(Slot::Value(v)) => Seen::Value(v.clone()),
                _ => Seen::Absent,
            };
        }
    }
    // A function's input: what its call was given, read where the call is.
    if let (ExprKind::Name(n), Some(f), Some((parent, k))) = (&part.kind, &ctx.function, &ctx.parent) {
        let i = w.found.inputs.get(f).and_then(|ins| ins.iter().position(|p| p == n));
        let span = parent.children.get(*k as usize).and_then(|id| w.lowered.spans.get(id));
        let call = span.and_then(|sp| w.lowered.sites.iter().find(|s| s.does == "enters" && s.at == *sp));
        if let Some(given) = i.and_then(|i| call.and_then(|c| c.needs.get(i))) {
            return crate::lang::reached::value(given, &mut |p| seen_need(w, parent, run, p));
        }
    }
    Seen::Unknown
}

/// A site's path on a run, in context `ctx`.
fn path_on(w: &Watched, site: &Site, ctx: &Context, run: &Run) -> Reached {
    for (i, atom) in site.path.iter().enumerate() {
        let ok = match atom {
            Atom::Entry => Some(true),
            // As today: outside a try a statement before always counts as completed (a run past
            // one that gave nothing is labelled by `PAST_NOTHING`).
            Atom::Completed(_) => Some(true),
            Atom::Entered(_) => match &ctx.parent {
                // The call this body was entered by: reached where its own site is.
                Some((parent, k)) => {
                    let span = parent.children.get(*k as usize).and_then(|id| w.lowered.spans.get(id));
                    let call = span.and_then(|sp| w.lowered.sites.iter().find(|s| s.does == "enters" && s.at == *sp));
                    match call.map(|c| path_on(w, c, parent, run)) {
                        Some(Reached::Yes) => Some(true),
                        Some(Reached::No) => Some(false),
                        _ => None,
                    }
                }
                None => None,
            },
            // A function written in place: its steps aren't reported, item by item.
            Atom::Item { .. } => None,
            Atom::Holds { test, .. } => holds(test, &mut |part| seen(w, ctx, run, part)),
        };
        match ok {
            Some(true) => {}
            Some(false) => return Reached::No,
            None => return Reached::NotKnown(i),
        }
    }
    Reached::Yes
}

/// The site's own steps in a context: its children written exactly where the site is, or (a
/// statement's) within it and within no other site of it.
fn own_steps(w: &Watched, site: &Site, ctx: &Context, store: &impl Resolver, unit: Option<&Unit>) -> Vec<u32> {
    let span_of = |id: &str| w.lowered.spans.get(id).copied();
    // A write to state: the step that writes its key, wherever the lowering put it (at the
    // statement around the places that write it).
    if let (Some(key), Some(Unit { body: UnitBody::Composite { children, .. }, .. })) = (&site.key, unit) {
        let writes = |k: usize| {
            children.get(k).and_then(|h| store.resolve(h)).is_some_and(|u| {
                matches!(&u.body, UnitBody::Primitive { name } if name == "store_write") && u.params.iter().any(|p| p.name() == "key" && p.value() == Some(key.as_str()))
            })
        };
        return ctx.children.iter().enumerate().filter(|(k, id)| writes(*k) && span_of(id).is_some_and(|sp| within(site.at, sp))).map(|(k, _)| k as u32).collect();
    }
    let exact: Vec<u32> = ctx.children.iter().enumerate().filter(|(_, id)| span_of(id) == Some(site.at)).map(|(k, _)| k as u32).collect();
    if !exact.is_empty() {
        return exact;
    }
    let others: Vec<Span> = w.lowered.sites.iter().filter(|s| s.at != site.at && within(s.at, site.at)).map(|s| s.at).collect();
    ctx.children
        .iter()
        .enumerate()
        .filter(|(_, id)| span_of(id).is_some_and(|sp| within(sp, site.at) && !others.iter().any(|o| within(sp, *o))))
        .map(|(k, _)| k as u32)
        .collect()
}

/// The contexts a run went into under `ctx`: the handler, then each function call it entered,
/// with the function's children.
fn contexts(w: &Watched, store: &impl Resolver, unit: &Unit, ctx: Context, run: &Run, out: &mut Vec<(Context, Option<String>)>, function: Option<String>) {
    out.push((ctx.clone(), function));
    let UnitBody::Composite { children, .. } = &unit.body else { return };
    for (k, h) in children.iter().enumerate() {
        let Some(ids) = w.found.holons.get(&hex(h)) else { continue };
        let mut path = ctx.path.clone();
        path.push(k as u32);
        if !run.entered.contains(&path) {
            continue;
        }
        let Some(child) = store.resolve(h) else { continue };
        let name = ids.first().and_then(|id| id.split('.').next()).map(str::to_string);
        let inner = Context { path, children: in_run_order(child, ids), parent: Some((Box::new(ctx.clone()), k as u32)), function: name.clone() };
        contexts(w, store, child, inner, run, out, name);
    }
}

/// Whether a statement before `site`, outside a try, gave nothing at a step that throws in
/// JavaScript, in context `ctx`.
fn past_nothing(w: &Watched, site: &Site, ctx: &Context, run: &Run, store: &impl Resolver, unit: &Unit) -> bool {
    let UnitBody::Composite { children, .. } = &unit.body else { return false };
    site.path.iter().any(|a| {
        let Atom::Completed(stmt) = a else { return false };
        ctx.children.iter().enumerate().any(|(k, id)| {
            let leaf = children.get(k).and_then(|h| store.resolve(h)).and_then(|u| if let UnitBody::Primitive { name } = &u.body { Some(name.as_str()) } else { None });
            let mut at = ctx.path.clone();
            at.push(k as u32);
            leaf.is_some_and(|l| THROWS_IN_JAVASCRIPT.contains(&l))
                && w.lowered.spans.get(id).is_some_and(|sp| within(*sp, *stmt))
                && run.produced.get(&at).is_some_and(|outs| outs.iter().all(|o| !matches!(o, Slot::Value(_))))
                && (run.entered.contains(&at) || run.asked.contains(&at) || leaf == Some("json_parse"))
        })
    })
}

/// What a value a site needs gives. A record or a list written in place that the lowering made no
/// step of (its fields went into the leaf's own settings, or one step each) has a value where each
/// of its parts has one, as a record with a field that has none has none.
fn seen_need(w: &Watched, ctx: &Context, run: &Run, need: &Expr) -> Seen {
    let whole = seen(w, ctx, run, need);
    if whole != Seen::Unknown {
        return whole;
    }
    let inner: Vec<&Expr> = match &need.kind {
        ExprKind::Record { spread, fields } => spread.iter().map(|s| &**s).chain(fields.iter().map(|(_, e)| e)).filter(|e| !matches!(e.kind, ExprKind::Lambda { .. })).collect(),
        ExprKind::List(xs) => xs.iter().collect(),
        _ => return whole,
    };
    let seen: Vec<Seen> = inner.into_iter().map(|e| crate::lang::reached::value(e, &mut |p| seen_need(w, ctx, run, p))).collect();
    if seen.contains(&Seen::Absent) {
        Seen::Absent
    } else if seen.contains(&Seen::Unknown) {
        Seen::Unknown
    } else {
        Seen::Value(Value::Boolean(true))
    }
}

/// What a handler is given on one run: its inputs, and what the host supplies its sourced leaves,
/// by leaf (`file_arrival`, `mail_arrival`, `feed_event`, `queue_message`, …).
pub type Given = (Vec<Option<Value>>, BTreeMap<String, Value>);

/// The store, with what the host supplies a run's sourced leaves.
struct Sourcing<'a> {
    store: &'a MapResolver,
    sourced: &'a BTreeMap<String, Value>,
}

impl Resolver for Sourcing<'_> {
    fn resolve(&self, hash: &holon_core::Hash) -> Option<&Unit> {
        self.store.resolve(hash)
    }
    fn sourced(&self, unit: &Unit) -> Option<&Value> {
        match &unit.body {
            UnitBody::Primitive { name } => self.sourced.get(name),
            _ => None,
        }
    }
}

/// Every site of `w` on every run of every handler: each handler run on each of `given(handler)`,
/// its asks answered by `answer`.
pub fn site_runs(w: &Watched, given: &dyn Fn(&Handler, &Unit) -> Vec<Given>, answer: fn(&Value) -> Answer) -> Vec<SiteRun> {
    let mut store = MapResolver::new();
    let hs = handlers(w, &mut store);
    let mut out = Vec::new();
    for h in &hs {
        let hash = parse_hash(&h.hash).expect("a hash");
        let unit = store.resolve(&hash).expect("the handler's unit").clone();
        // The handler's own composite, inside its endpoint's (or its clock's, …) unit.
        let UnitBody::Composite { children, .. } = &unit.body else { continue };
        let Some(handler) = children.first().and_then(|c| store.resolve(c)).cloned() else { continue };
        // The sites written within this handler's body, and those of the functions it calls.
        let body = |s: &Site| h.children.iter().any(|id| w.lowered.spans.get(id).is_some_and(|sp| within(*sp, s.within)));
        for (i, (input, sourced)) in given(h, &unit).into_iter().enumerate() {
            let mut run = Run { answer: Some(answer), ..Run::default() };
            eval_observed(&unit, &input, &Sourcing { store: &store, sourced: &sourced }, &mut run);
            let mut ctxs = Vec::new();
            contexts(w, &store, &handler, Context { path: vec![0], children: in_run_order(&handler, &h.children), parent: None, function: None }, &run, &mut ctxs, None);
            for (ctx, function) in &ctxs {
                let ctx_unit = resolve_at(&store, &unit, &ctx.path);
                for (n, site) in w.lowered.sites.iter().enumerate() {
                    let here = match (function, site.path.first()) {
                        (Some(f), Some(Atom::Entered(g))) => f == g,
                        (None, Some(Atom::Entry)) => body(site),
                        _ => false,
                    };
                    if !here {
                        continue;
                    }
                    let steps = own_steps(w, site, ctx, &store, ctx_unit.as_ref());
                    let today = (!steps.is_empty()).then(|| {
                        steps.iter().any(|k| {
                            let mut at = ctx.path.clone();
                            at.push(*k);
                            does_something(&run, &at)
                        })
                    });
                    let kept = (!site.kept.is_empty()).then(|| site.kept.iter().all(|t| holds(t, &mut |p| seen(w, ctx, &run, p)) == Some(true)));
                    let past = ctx_unit.as_ref().is_some_and(|u| past_nothing(w, site, ctx, &run, &store, u));
                    // One with none is enough to say it has none; else one never computed leaves
                    // it not known.
                    let values: Vec<Seen> = site.needs.iter().map(|need| crate::lang::reached::value(need, &mut |p| seen_need(w, ctx, &run, p))).collect();
                    let has_values = if values.contains(&Seen::Absent) {
                        Some(false)
                    } else if values.contains(&Seen::Unknown) {
                        None
                    } else {
                        Some(true)
                    };
                    let one_key = site.key.is_some() && w.lowered.sites.iter().filter(|s| s.key == site.key && s.within == site.within).count() > 1;
                    out.push(SiteRun { site: n, handler: h.label.clone(), input: i, at: ctx.path.clone(), reached: path_on(w, site, ctx, &run), today, kept, has_values, past_nothing: past, one_key });
                }
            }
        }
    }
    out
}

/// A composite's children's nodes in the order a run in this store goes through them. A trace
/// (`flow::trace_of`) names them in the canonical order the server's store hands a unit back in;
/// an in-memory store keeps the order they were made in, which canonical position `p` is child
/// `order[p]` of.
fn in_run_order(unit: &Unit, trace: &[String]) -> Vec<String> {
    let UnitBody::Composite { children, edges } = &unit.body else { return trace.to_vec() };
    let Ok(order) = holon_core::canonical_child_order(children, edges) else { return trace.to_vec() };
    let mut out = vec![String::new(); children.len()];
    for (p, &k) in order.iter().enumerate() {
        if let (Some(slot), Some(id)) = (out.get_mut(k), trace.get(p)) {
            *slot = id.clone();
        }
    }
    out
}

/// The unit at `path` below `root`.
fn resolve_at(store: &impl Resolver, root: &Unit, path: &[u32]) -> Option<Unit> {
    let mut u = root.clone();
    for k in path {
        let UnitBody::Composite { children, .. } = &u.body else { return None };
        u = store.resolve(children.get(*k as usize)?)?.clone();
    }
    Some(u)
}

/// A request as the data plane gives a route one (golden.rs's).
pub fn request(method: &str, path: &str, body: &str) -> Value {
    let text = |s: &str| Value::Text(s.into());
    let params: BTreeMap<String, Value> = path.split('/').filter_map(|seg| seg.strip_prefix(':').or_else(|| seg.strip_prefix('*'))).map(|k| (k.to_string(), text("tea"))).collect();
    Value::Record(BTreeMap::from([
        ("method".to_string(), text(method)),
        ("path".to_string(), text(path)),
        ("params".to_string(), Value::Record(params)),
        ("query".to_string(), Value::Record(BTreeMap::from([("after".to_string(), text("a"))]))),
        ("headers".to_string(), Value::Record(BTreeMap::from([("content-type".to_string(), text("application/json"))]))),
        ("body".to_string(), text(body)),
    ]))
}

/// A host's answer as a test gives it: a request answered 200 with an empty body, a folder's file
/// its handle, another flow's action called a record; any other ask (a collection's or a
/// database's read, a start, a wait) nothing.
pub fn answer_ok(request: &Value) -> Answer {
    match request {
        Value::Record(r) if r.contains_key("url") => Answer::Value(Value::Record(BTreeMap::from([
            ("status".to_string(), Value::Number(200.0)),
            ("headers".to_string(), Value::Record(BTreeMap::new())),
            ("body".to_string(), Value::Text(String::new())),
        ]))),
        // A folder's file asked for by name: there, its handle.
        Value::Record(r) if r.contains_key("handle") && r.contains_key("folder") => {
            let name = r.get("handle").cloned().unwrap_or(Value::Text(String::new()));
            Answer::Value(Value::Record(BTreeMap::from([
                ("folder".to_string(), r["folder"].clone()),
                ("name".to_string(), name),
                ("size".to_string(), Value::Number(4.0)),
                ("hash".to_string(), Value::Text("9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into())),
                ("modified".to_string(), Value::Number(1_763_856_000.0)),
            ])))
        }
        // Another flow's action called (not started) answers what it returned: here, a record.
        Value::Record(r) if r.contains_key("flow") && !r.contains_key("start") => Answer::Value(Value::Record(BTreeMap::new())),
        _ => Answer::Absent,
    }
}

#[cfg(test)]
mod tests;

/// The bodies each handler is given, golden's three (golden.rs reads them here): none, a record
/// with the fields samples read, and a list.
pub fn bodies() -> Vec<String> {
    let xs: Vec<String> = (0..60).map(|i| format!("{}.25", (i * 37) % 100)).collect();
    vec![
        String::new(),
        r#"{"id":"x","name":"tea","price":4,"amount":5,"qty":2,"items":[{"name":"tea","qty":2},{"name":"cake","qty":1}],"event":"gig","count":2,"text":"Hello World","title":"Hello World","author":"Ada","op":"mul","a":6,"b":7,"by":"ops","to":5}"#.to_string(),
        format!(r#"{{"xs":[{}],"items":[]}}"#, xs.join(",")),
    ]
}

fn record(pairs: &[(&str, Value)]) -> Value {
    Value::Record(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

/// What a handler is given on each of golden's bodies ([`given_on`]).
pub fn given(h: &Handler, unit: &Unit) -> Vec<Given> {
    given_on(h, unit, &bodies())
}

/// What a handler of `kind` is given on each of `bodies`, the host's records in the shapes the
/// catalog documents (holon-core docs.rs): a route its request, an action the body as a value, a
/// websocket's message its connection's record with the body as its text (websockets.rs); a file's
/// handler its file's handle, a mail's its message (the body as its text), a feed's its event and
/// a queue's its message (the body as their data); every run its start time; state empty, as on a
/// first run.
pub fn given_on(h: &Handler, unit: &Unit, bodies: &[String]) -> Vec<Given> {
    bodies
        .iter()
        .map(|b| {
            let value = holon_core::parse_json(if b.is_empty() { "{}" } else { b }).unwrap_or(Value::Record(BTreeMap::new()));
            let mut sourced = BTreeMap::new();
            let input = match h.kind {
                "route" => vec![Some(request(&h.method, &h.path, b))],
                "websocket" => {
                    let message = record(&[("conn", text("c1")), ("path", text(&h.path)), ("params", Value::Record(BTreeMap::new())), ("query", Value::Record(BTreeMap::new())), ("text", text(b))]);
                    vec![Some(message); unit.inputs.len()]
                }
                _ => vec![Some(value.clone()); unit.inputs.len()],
            };
            let n = |x: f64| Value::Number(x);
            // Every run has its start time, as the host supplies it; state is empty, as on a
            // deployment's first run.
            sourced.insert("clock".to_string(), n(1_763_856_000.0));
            match h.kind {
                "file" => {
                    let handle = record(&[("folder", text(&h.path)), ("name", text("a.csv")), ("size", n(b.len() as f64)), ("hash", text("9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08")), ("modified", n(1_763_856_000.0)), ("event", text("new"))]);
                    sourced.insert("file_arrival".to_string(), handle);
                }
                "mail" => {
                    let mail = record(&[
                        ("from", text("ada@example.com")),
                        ("name", text("Ada")),
                        ("to", Value::List(vec![text("ops@example.com")])),
                        ("cc", Value::List(vec![])),
                        ("subject", text("pay")),
                        ("date", text("2026-11-23T10:00:00Z")),
                        ("message_id", text("<1@example.com>")),
                        ("text", text(b)),
                        ("html", text("")),
                        ("attachments", Value::Record(BTreeMap::new())),
                        ("authenticated", Value::Boolean(true)),
                        ("verdict", text("pass")),
                    ]);
                    sourced.insert("mail_arrival".to_string(), mail);
                }
                "feed" => {
                    sourced.insert("feed_event".to_string(), record(&[("data", value.clone()), ("id", text("1")), ("event", text("message"))]));
                }
                "queue" => {
                    let message = record(&[("data", value.clone()), ("id", text("m1")), ("attributes", Value::Record(BTreeMap::new())), ("sent", n(1_763_856_000.0)), ("count", n(1.0)), ("group", text(""))]);
                    sourced.insert("queue_message".to_string(), message);
                }
                _ => {}
            }
            (input, sourced)
        })
        .collect()
}
