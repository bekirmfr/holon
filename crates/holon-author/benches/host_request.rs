//! A deployed flow's request, in-process, the way the server runs it: the sample compiled from its
//! text, its server unit read, the route's endpoint run through `holon_host::run_rounds` with the
//! request record a listener makes (crates/holon/src/data.rs `request_record`), under a resolver
//! that keeps every unit's shape (crates/holon/src/units.rs `Loaded`) and an observer that times
//! each node down to three levels and keeps what it produced (crates/holon/src/recorder.rs
//! `Timer`). Outbound requests answer from a stub, collection reads with a number, effects are
//! only recorded: what is timed is the evaluator and the host's rounds around it.
//!
//! Each sample prints one row, `| request <sample> <route> | 1 | ns |`, timed as the races time.
//! A filter argument runs the samples whose row has it. Built against the public API alone, so it
//! races the evaluator before the change against after it.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::{Duration, Instant};

use holon_author::composites::Names;
use holon_author::flow::flow_unit;
use holon_core::{read_server, Capability, Hash, MapResolver, Observer, Resolver, Shape, Shapes, Slot, Unit, Value};
use holon_host::{run_rounds, Host, Limits, Performer, Waits};

/// The server's resolver: every unit's shape kept.
struct Keeping {
    store: MapResolver,
    shapes: Shapes,
}

impl Resolver for Keeping {
    fn resolve(&self, hash: &Hash) -> Option<&Unit> {
        self.store.resolve(hash)
    }
    fn shape(&self, unit: &Unit) -> Option<&Shape> {
        self.shapes.get(unit)
    }
}

/// A node's entry in the timer's timeline: its path, when it started and ended, what it produced.
type Timed = (Vec<u32>, f64, f64, Option<Vec<Option<Value>>>);

/// The recorder's timer, as far as the evaluator sees it: a clock read at each node's start and
/// end down to three levels, and its outputs kept.
struct Timer {
    start: Instant,
    open: Vec<f64>,
    produced: Option<Vec<Option<Value>>>,
    timeline: Vec<Timed>,
}

impl Timer {
    fn new() -> Timer {
        Timer { start: Instant::now(), open: Vec::new(), produced: None, timeline: Vec::new() }
    }
    fn now_us(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1e6
    }
}

impl Observer for Timer {
    fn enter(&mut self, path: &[u32]) {
        if path.len() <= 3 {
            let t = self.now_us();
            self.open.push(t);
        }
    }
    fn produced(&mut self, path: &[u32], outputs: &[Slot]) {
        if path.len() <= 3 {
            self.produced = Some(outputs.iter().map(|o| o.value().cloned()).collect());
        }
    }
    fn leave(&mut self, path: &[u32]) {
        if path.len() <= 3 {
            let start = self.open.pop().unwrap_or(0.0);
            let end = self.now_us();
            let outputs = self.produced.take();
            self.timeline.push((path.to_vec(), start, end, outputs));
        }
    }
}

/// Effects recorded, never performed.
struct Recording(Vec<(Capability, Value)>);

impl Host for Recording {
    fn perform(&mut self, capability: Capability, payload: &Value) {
        self.0.push((capability, payload.clone()));
    }
}

/// Every outbound request answered at once with the same JSON page.
struct Stub;

impl Performer for Stub {
    fn perform(&self, _request: &Value, _limits: &Limits) -> Value {
        Value::Record(BTreeMap::from([
            ("status".to_string(), Value::Number(200.0)),
            ("headers".to_string(), Value::Record(BTreeMap::new())),
            ("body".to_string(), Value::Text("{\"slideshow\": {\"title\": \"Sample Slide Show\"}}".to_string())),
        ]))
    }
}

/// A run with nothing delivered, whose collection reads each answer 4.
struct Reads;

impl Waits for Reads {
    fn event(&self, _: &str) -> Option<Value> {
        None
    }
    fn token(&mut self, _: &str) -> String {
        String::new()
    }
    fn read(&mut self, _path: &[u32], _request: &Value) -> Option<Value> {
        Some(Value::Number(4.0))
    }
}

/// A sample's route, ready to run: the flow's units, and the endpoint's hash.
struct Route {
    resolver: Keeping,
    endpoint: Hash,
}

fn route(text: &str, method: &str, path: &str) -> Route {
    let lowered = holon_author::lang::ts::compile(text).unwrap_or_else(|e| panic!("{e:?}"));
    let mut store = MapResolver::new();
    holon_author::text::restore(&lowered.doc, &mut store, &Names::new());
    let port = lowered.doc.server.as_ref().map(|s| s.port.clone()).unwrap_or_default();
    let server = flow_unit(&lowered.doc.nodes, &lowered.doc.edges, &mut store, &port).unwrap_or_else(|e| panic!("{e}"));
    let server = read_server(store.resolve(&server).expect("stored"), &store).unwrap_or_else(|e| panic!("{e:?}"));
    let endpoint = server.routes.iter().find(|r| r.method == method && r.path == path).unwrap_or_else(|| panic!("no route {method} {path}")).endpoint;
    let shapes = Shapes::of(store.units());
    Route { resolver: Keeping { store, shapes }, endpoint }
}

/// The record a listener makes of a request (data.rs `request_record`).
fn request(method: &str, path: &str, params: &[(&str, &str)], body: &str) -> Value {
    let texts = |kv: &[(&str, &str)]| Value::Record(kv.iter().map(|(k, v)| (k.to_string(), Value::Text(v.to_string()))).collect());
    Value::Record(BTreeMap::from([
        ("method".to_string(), Value::Text(method.to_string())),
        ("path".to_string(), Value::Text(path.to_string())),
        ("params".to_string(), texts(params)),
        ("query".to_string(), texts(&[])),
        ("headers".to_string(), texts(&[("content-type", "application/json")])),
        ("body".to_string(), Value::Text(body.to_string())),
    ]))
}

fn time(mut once: impl FnMut()) -> f64 {
    let mut calls = 1u64;
    loop {
        let start = Instant::now();
        for _ in 0..calls {
            once();
        }
        if start.elapsed() >= Duration::from_millis(20) || calls >= 1 << 30 {
            break;
        }
        calls *= 2;
    }
    let mut samples: Vec<f64> = (0..7)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..calls {
                once();
            }
            start.elapsed().as_nanos() as f64 / calls as f64
        })
        .collect();
    samples.sort_by(|x, y| x.partial_cmp(y).unwrap());
    samples[3]
}

fn wanted(name: &str) -> bool {
    match std::env::args().skip(1).find(|a| !a.starts_with('-')) {
        Some(filter) => name.contains(&filter),
        None => true,
    }
}

/// One request through `run_rounds`, as the server makes it, timed; panics if it doesn't answer.
fn run(name: &str, r: &Route, req: Value) {
    if !wanted(name) {
        return;
    }
    let root = r.resolver.resolve(&r.endpoint).expect("the endpoint resolves");
    let inputs = [Some(req)];
    let sourced = [(Capability::Clock, Value::Number(1_762_400_000.0)), (Capability::Random, Value::Number(0.5))];
    let limits = Limits::request_run();
    let once = || {
        let mut host = Recording(Vec::new());
        let mut timer = Timer::new();
        let out = run_rounds(root, &inputs, &sourced, &r.resolver, &mut host, &Stub, &limits, &mut timer, &mut Reads, &[]);
        (out, host, timer)
    };
    let (out, _, timer) = once();
    assert!(out.error.is_none() && out.outputs.first().is_some_and(Option::is_some), "{name}: {out:?}");
    assert!(!timer.timeline.is_empty(), "{name}: the timer saw no node");
    println!("| {name} | 1 | {:.1} |", time(|| {
        black_box(once());
    }));
}

fn main() {
    let hello = route(include_str!("../samples/hello-api.holon.ts"), "GET", "/hello/:name");
    run("request hello-api GET /hello/:name", &hello, request("GET", "/hello/ada", &[("name", "ada")], ""));

    let calc = route(include_str!("../samples/calc.holon.ts"), "POST", "/calc");
    run("request calc POST /calc", &calc, request("POST", "/calc", &[], "{\"op\": \"mul\", \"a\": 6, \"b\": 7}"));

    let quote = route(include_str!("../samples/quote.holon.ts"), "POST", "/quote");
    let items: Vec<String> = (0..20).map(|k| format!("{{\"name\": \"item{k}\", \"price\": {}, \"qty\": {}}}", k + 1, k % 3 + 1)).collect();
    run("request quote POST /quote (20 items)", &quote, request("POST", "/quote", &[], &format!("{{\"tax\": 0.2, \"items\": [{}]}}", items.join(", "))));

    let basket = route(include_str!("../samples/basket.holon.ts"), "POST", "/basket");
    let lines: Vec<String> = (0..10).map(|k| format!("{{\"name\": \"item{k}\", \"qty\": {}}}", k % 3 + 1)).collect();
    run("request basket POST /basket (10 reads)", &basket, request("POST", "/basket", &[], &format!("{{\"items\": [{}]}}", lines.join(", "))));

    let fetch = route(include_str!("../samples/fetch-json.holon.ts"), "GET", "/slideshow");
    run("request fetch-json GET /slideshow (stub)", &fetch, request("GET", "/slideshow", &[], ""));

    let slugs = route(include_str!("../samples/slugs.holon.ts"), "POST", "/slugs");
    run("request slugs POST /slugs", &slugs, request("POST", "/slugs", &[], "{\"title\": \"Hello Holon World\", \"author\": \"Ada Lovelace\"}"));
}
