//! `holon load`: a deployed flow's routes under load, one after another, from connections kept
//! open, and what the server did with their runs. Each route is sent requests as fast as it
//! answers them for a while; each second it says how many it answered and how fast; at the end,
//! for each route, the requests a second, the latency (p50, p90, p99, the slowest), the errors,
//! and from the server: the runs it recorded, the times it synced its log to disk for them, and
//! the runs it only counted. The `stress-test` sample has one route of each log level, to see
//! what each costs.
//!
//! The requests are plain HTTP/1.1 on sockets of its own, a small client rather than `ureq`, so
//! the load costs the machine little beside the server it shares it with, and a latency is the
//! request's alone. Latencies are kept in a histogram of fixed size ([`Hist`]), so a long run
//! holds no more than a short one.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cmdline::{self, seg, Args, SIGN_IN};

const WHO: &str = "holon load";

fn usage() -> String {
    format!(
        "usage: holon load <slug> [options]

  Load-tests a deployed flow: sends each of its routes as many requests as it can answer,
  one route at a time, then reports how fast it answered and what the server did with the runs.

  --route \"METHOD /path\"   a route to load; repeat it for more, loaded in the order given.
                           Default: every GET route, strict first, then lazy, then unrecorded.
                           A path parameter is sent as 1 unless you give it (GET /items/42).
  --for SECONDS            how long to load each route (default 10, at most 3600)
  --connections N          connections kept open, each sending its next request as soon as
                           the last one is answered (default 8, at most 1024)
  --body JSON              the body sent to each route other than GET (default {{}})
  --to http://HOST:PORT    where the flow answers, if not at its port on the server's host
                           (a proxy in front, say); plain HTTP only
  --json                   print the results as JSON, with each second's numbers
  --server PORT|URL        which holon server to ask (see below)

  While it runs, it prints a line a second for the route being loaded: requests answered,
  p50 and p99. At the end, a table with a row per route:

    req/s, p50 … max   requests answered a second, and how long they took
    errors             answers other than 2xx, and requests that got no answer
    recorded           runs the server wrote to the flow's log
    syncs              times the server synced that log to disk (one sync covers every
                       run written before it)
    counted            runs of an unrecorded route: counted, not logged

  The server's counts cover the whole deployment, so other traffic during the test shows up
  in them. Run on the same machine, the load shares its processors with the server: compare
  routes with each other rather than reading the numbers on their own. Exits 1 when a route
  never answered.

examples:
  holon load stress-test
  holon load stress-test --for 5 --connections 32
  holon load stress-test --route \"GET /strict\" --route \"GET /unrecorded\"
  holon load notes-api --route \"PUT /notes/7\" --body '{{\"text\":\"hi\"}}'
  holon load stress-test --json > load.json

{SIGN_IN}"
    )
}

/// `holon load`, with `args` after the command.
pub(crate) fn cli(args: &[String]) -> i32 {
    let usage = usage();
    let args = match cmdline::parse(WHO, &usage, args, &["--route", "--for", "--connections", "--body", "--to"], &["--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    match run(&args) {
        Ok(()) => 0,
        Err(Stop::Said(code)) => code,
        Err(Stop::Mistake(why)) => {
            eprintln!("{WHO}: {why} (holon load --help says how)");
            2
        }
        Err(Stop::Failed(why)) => {
            eprintln!("{WHO}: {why}");
            1
        }
    }
}

/// Why it stopped: already said (an exit code), a mistake in what was asked, or something else.
enum Stop {
    Said(i32),
    Mistake(String),
    Failed(String),
}

impl From<i32> for Stop {
    fn from(code: i32) -> Stop {
        Stop::Said(code)
    }
}

/// One route to load: what is sent, and how the flow records its runs.
#[derive(Clone, Debug, PartialEq)]
struct Route {
    method: String,
    /// As declared (`/todos/:id`): what the server's tallies name it by.
    declared: String,
    /// As sent (`/todos/1`).
    path: String,
    /// `strict`, `lazy` or `unrecorded`, as granted.
    log: String,
}

impl Route {
    fn label(&self) -> String {
        format!("{} {}", self.method, self.declared)
    }

    /// As each second's line names it: as sent, and how it is recorded.
    fn named(&self) -> String {
        format!("{} {} ({})", self.method, self.path, self.log)
    }
}

fn run(args: &Args) -> Result<(), Stop> {
    let Some(slug) = args.word(0) else { return Err(Stop::Mistake("which deployment? holon load <slug>".into())) };
    if args.words.len() > 1 {
        return Err(Stop::Mistake(format!("one deployment at a time: {:?} is one too many", args.words[1])));
    }
    let seconds = number(args.value("--for"), 10, 1, 3600, "--for takes a number of seconds, 1 to 3600")?;
    let connections = number(args.value("--connections"), 8, 1, 1024, "--connections takes a number, 1 to 1024")? as usize;
    let body = args.value("--body").map(str::to_string);
    if let Some(b) = &body {
        serde_json::from_str::<Value>(b).map_err(|e| Stop::Mistake(format!("--body isn't JSON: {e}")))?;
    }

    let listed = cmdline::json(WHO, args, "GET", "/deployments", None)?;
    let Some(dep) = listed["deployments"].as_array().and_then(|all| all.iter().find(|d| d["slug"] == slug)).cloned() else {
        return Err(Stop::Failed(format!("no deployment {slug:?} (holon deployments lists them)")));
    };
    if dep["status"] != "running" {
        return Err(Stop::Failed(format!("{slug} is {}: holon run {slug} starts it", dep["status"].as_str().unwrap_or("not running"))));
    }
    let declared: Vec<(String, String, String)> = dep["routes"]
        .as_array()
        .map(|rs| rs.iter().map(|r| (text(&r["method"]), text(&r["path"]), text(&r["log"]))).collect())
        .unwrap_or_default();
    let routes = pick_routes(&declared, &args.all("--route"))?;
    let base = match args.value("--to") {
        Some(to) => Base::parse(to).map_err(Stop::Mistake)?,
        None => {
            let Some(port) = dep["port"].as_u64() else { return Err(Stop::Failed(format!("{slug} has no port: it answers no requests"))) };
            let server = crate::local::target(args.server()).map_err(Stop::Failed)?.server;
            Base { host: host_of(&server), port: port as u16 }
        }
    };
    let addr = base.addr().map_err(Stop::Failed)?;

    let json_out = args.flag("--json");
    eprintln!(
        "{WHO}: {slug} at {}, {} route{}, {seconds} s each, {connections} connection{}",
        base.authority(),
        routes.len(),
        if routes.len() == 1 { "" } else { "s" },
        if connections == 1 { "" } else { "s" }
    );
    let log_path = format!("/deployments/{}/log?limit=0", seg(slug));
    let width = routes.iter().map(|r| r.named().chars().count()).max().unwrap_or(0);
    let mut results = Vec::new();
    for (i, route) in routes.iter().enumerate() {
        let before = Counts::read(args, &log_path, &route.label())?;
        let request = request_bytes(route, &base, body.as_deref());
        let loaded = load(addr, &request, connections, Duration::from_secs(seconds), |s, second| {
            eprintln!("{}", second_line(route, width, s, seconds, second));
        });
        // A lazy route's runs are on disk within a second of their answers: wait that out, so its
        // syncs are counted, and so the next route starts on a server done with this one's.
        std::thread::sleep(crate::recorder::SYNC_EVERY + Duration::from_millis(300));
        let after = Counts::read(args, &log_path, &route.label())?;
        results.push(Outcome { route: route.clone(), loaded, server: after.since(&before) });
        if i + 1 < routes.len() {
            eprintln!();
        }
    }
    // A route nothing answered was never loaded: said, and the exit code says so.
    let unanswered: Vec<String> = results.iter().filter(|r| r.loaded.total.hist.n == 0).map(|r| r.route.label()).collect();
    if json_out {
        cmdline::print_json(&json!({
            "slug": slug,
            "at": base.authority(),
            "seconds": seconds,
            "connections": connections,
            "routes": results.iter().map(Outcome::json).collect::<Vec<_>>(),
        }));
    } else {
        println!();
        for line in summary(&results) {
            println!("{line}");
        }
    }
    if !unanswered.is_empty() {
        return Err(Stop::Failed(format!("nothing answered {}", unanswered.join(", "))));
    }
    Ok(())
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn number(given: Option<&str>, default: u64, min: u64, max: u64, why: &str) -> Result<u64, Stop> {
    match given {
        None => Ok(default),
        Some(n) => n.trim().trim_end_matches('s').parse::<u64>().ok().filter(|n| (min..=max).contains(n)).ok_or_else(|| Stop::Mistake(why.into())),
    }
}

/// The routes to load, from those declared (`(method, path, log)`): each `--route` asked
/// (`"GET /x"`, a declared path or one with its parameters filled), in the order asked; or every
/// GET route, strict ones first, then lazy, then unrecorded.
fn pick_routes(declared: &[(String, String, String)], asked: &[&str]) -> Result<Vec<Route>, Stop> {
    if declared.is_empty() {
        return Err(Stop::Failed("it has no routes".into()));
    }
    let route = |(method, path, log): &(String, String, String), sent: Option<&str>| Route {
        method: method.clone(),
        declared: path.clone(),
        path: sent.map(str::to_string).unwrap_or_else(|| filled(path)),
        log: log.clone(),
    };
    if asked.is_empty() {
        let mut gets: Vec<Route> = declared.iter().filter(|r| r.0 == "GET").map(|r| route(r, None)).collect();
        // The server lists them in an order of its own: strict first, then lazy, then unrecorded.
        let rank = |log: &str| ["strict", "lazy", "unrecorded"].iter().position(|l| *l == log).unwrap_or(3);
        gets.sort_by(|a, b| rank(&a.log).cmp(&rank(&b.log)).then_with(|| a.declared.cmp(&b.declared)));
        if gets.is_empty() {
            return Err(Stop::Failed("it has no GET route: name one to load with --route \"METHOD /path\"".into()));
        }
        return Ok(gets);
    }
    asked
        .iter()
        .map(|a| {
            let (method, path) = a.trim().split_once(' ').map(|(m, p)| (m.to_ascii_uppercase(), p.trim())).ok_or_else(|| Stop::Mistake(format!("--route {a:?}: give a method and a path, \"GET /x\"")))?;
            let found = declared.iter().find(|r| r.0 == method && r.1 == path).map(|r| route(r, None));
            let found = found.or_else(|| declared.iter().find(|r| r.0 == method && matches(&r.1, path)).map(|r| route(r, Some(path))));
            found.ok_or_else(|| {
                let all: Vec<String> = declared.iter().map(|r| format!("{} {}", r.0, r.1)).collect();
                Stop::Failed(format!("no route {method} {path}: it has {}", all.join(", ")))
            })
        })
        .collect()
}

/// A declared path with each parameter (`:id`, `*rest`) as `1`.
fn filled(path: &str) -> String {
    path.split('/').map(|s| if s.starts_with(':') || s.starts_with('*') { "1" } else { s }).collect::<Vec<_>>().join("/")
}

/// Whether `sent` is a path the declared `path` answers.
fn matches(path: &str, sent: &str) -> bool {
    let (d, s): (Vec<&str>, Vec<&str>) = (path.split('/').collect(), sent.split('/').collect());
    if d.last().is_some_and(|l| l.starts_with('*')) {
        return s.len() >= d.len() && d.iter().zip(&s).all(|(d, s)| d.starts_with(':') || d.starts_with('*') || d == s);
    }
    d.len() == s.len() && d.iter().zip(&s).all(|(d, s)| d.starts_with(':') || d == s)
}

/// Where a flow answers: a host and a port.
#[derive(Debug, PartialEq)]
struct Base {
    host: String,
    port: u16,
}

impl Base {
    /// `http://HOST:PORT` (`--to`).
    fn parse(to: &str) -> Result<Base, String> {
        let to = to.trim().trim_end_matches('/');
        if to.starts_with("https://") {
            return Err("--to takes plain http://: holon load speaks no TLS (load the flow's own port, behind the proxy)".into());
        }
        let rest = to.strip_prefix("http://").unwrap_or(to);
        let (host, port) = rest.rsplit_once(':').filter(|(h, _)| !h.is_empty() && (!h.contains(':') || h.ends_with(']'))).ok_or_else(|| format!("--to {to:?}: give http://HOST:PORT"))?;
        let port = port.parse::<u16>().map_err(|_| format!("--to {to:?}: {port:?} isn't a port"))?;
        Ok(Base { host: host.trim_start_matches('[').trim_end_matches(']').to_string(), port })
    }

    /// As a `Host` header names it.
    fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    fn addr(&self) -> Result<SocketAddr, String> {
        (self.host.as_str(), self.port).to_socket_addrs().ok().and_then(|mut a| a.next()).ok_or_else(|| format!("can't find {}", self.host))
    }
}

/// The host of a server's URL (`http://127.0.0.1:7420` → `127.0.0.1`): a flow's port is there.
fn host_of(server: &str) -> String {
    let rest = server.split_once("://").map_or(server, |(_, r)| r);
    let authority = rest.split('/').next().unwrap_or(rest);
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    if let Some(v6) = authority.strip_prefix('[') {
        return v6.split(']').next().unwrap_or(v6).to_string();
    }
    authority.rsplit_once(':').map_or(authority, |(h, _)| h).to_string()
}

/// The request a route is sent, every time.
fn request_bytes(route: &Route, base: &Base, body: Option<&str>) -> Vec<u8> {
    let body = if route.method == "GET" || route.method == "HEAD" { None } else { Some(body.unwrap_or("{}")) };
    let mut r = format!("{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: holon-load\r\nAccept: */*\r\n", route.method, route.path, base.authority());
    if let Some(b) = body {
        r.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", b.len()));
    }
    r.push_str("\r\n");
    if let Some(b) = body {
        r.push_str(b);
    }
    r.into_bytes()
}

/// Latencies in microseconds, counted in buckets of fixed size: exact below 64 µs, then 64 to each
/// power of two (within 1.6%), up to 2^40 µs. Two add up ([`Hist::add`]), so the connections'
/// make the route's.
#[derive(Clone)]
struct Hist {
    counts: Vec<u64>,
    n: u64,
    sum_us: u64,
    max_us: u64,
}

const SUB_BITS: u32 = 6;
const SUB: u64 = 1 << SUB_BITS;
const TOP_BITS: u32 = 40;

impl Hist {
    fn new() -> Hist {
        Hist { counts: vec![0; ((TOP_BITS - SUB_BITS + 1) as usize) * SUB as usize], n: 0, sum_us: 0, max_us: 0 }
    }

    fn index(us: u64) -> usize {
        let us = us.min((1 << TOP_BITS) - 1);
        if us < SUB {
            return us as usize;
        }
        let e = 63 - us.leading_zeros();
        let m = (us >> (e - SUB_BITS)) & (SUB - 1);
        ((e - SUB_BITS + 1) as u64 * SUB + m) as usize
    }

    /// The middle of bucket `i`.
    fn value(i: usize) -> u64 {
        let i = i as u64;
        if i < SUB {
            return i;
        }
        let (e, m) = (i / SUB + SUB_BITS as u64 - 1, i % SUB);
        let low = (SUB + m) << (e - SUB_BITS as u64);
        low + (1 << (e - SUB_BITS as u64)) / 2
    }

    fn record(&mut self, us: u64) {
        self.counts[Hist::index(us)] += 1;
        self.n += 1;
        self.sum_us += us;
        self.max_us = self.max_us.max(us);
    }

    fn add(&mut self, other: &Hist) {
        for (a, b) in self.counts.iter_mut().zip(&other.counts) {
            *a += b;
        }
        self.n += other.n;
        self.sum_us += other.sum_us;
        self.max_us = self.max_us.max(other.max_us);
    }

    /// The latency `q` (0 to 1) of them are at or under, in µs; the slowest for 1.
    fn quantile(&self, q: f64) -> u64 {
        if self.n == 0 {
            return 0;
        }
        let rank = ((q * self.n as f64).ceil() as u64).clamp(1, self.n);
        let mut seen = 0;
        for (i, c) in self.counts.iter().enumerate() {
            seen += c;
            if seen >= rank {
                return Hist::value(i).min(self.max_us);
            }
        }
        self.max_us
    }
}

/// What a route's connections saw, in a second or all told.
#[derive(Clone)]
struct Seen {
    /// Every answer's latency, whatever its status.
    hist: Hist,
    /// Answers other than 2xx, by status.
    statuses: BTreeMap<u16, u64>,
    /// Requests with no answer (a connection refused, closed, timed out), and the last one's why.
    failed: u64,
    last_failure: Option<String>,
}

impl Seen {
    fn new() -> Seen {
        Seen { hist: Hist::new(), statuses: BTreeMap::new(), failed: 0, last_failure: None }
    }

    fn add(&mut self, other: &Seen) {
        self.hist.add(&other.hist);
        for (s, n) in &other.statuses {
            *self.statuses.entry(*s).or_default() += n;
        }
        self.failed += other.failed;
        if other.last_failure.is_some() {
            self.last_failure.clone_from(&other.last_failure);
        }
    }

    fn errors(&self) -> u64 {
        self.statuses.values().sum::<u64>() + self.failed
    }

    /// The answers that were 2xx.
    fn ok(&self) -> u64 {
        self.hist.n - self.statuses.values().sum::<u64>()
    }
}

/// A route loaded: what was seen each second, and all told over `elapsed`.
struct Loaded {
    seconds: Vec<Seen>,
    total: Seen,
    elapsed: Duration,
}

/// Send `request` to `addr` from `connections` connections, each on a thread of its own, for
/// `run`; `each` is told what each second saw, as it ends.
fn load(addr: SocketAddr, request: &[u8], connections: usize, run: Duration, mut each: impl FnMut(&Seen, usize)) -> Loaded {
    let stop = Arc::new(AtomicBool::new(false));
    let windows: Vec<Arc<Mutex<Seen>>> = (0..connections).map(|_| Arc::new(Mutex::new(Seen::new()))).collect();
    let request: Arc<[u8]> = request.into();
    let started = Instant::now();
    let workers: Vec<_> = windows
        .iter()
        .map(|w| {
            let (w, stop, request) = (Arc::clone(w), Arc::clone(&stop), Arc::clone(&request));
            std::thread::spawn(move || connection(addr, &request, &stop, &w))
        })
        .collect();
    let mut seconds = Vec::new();
    let mut total = Seen::new();
    let take = |seconds: &mut Vec<Seen>, total: &mut Seen| {
        let mut second = Seen::new();
        for w in &windows {
            let taken = std::mem::replace(&mut *w.lock().unwrap(), Seen::new());
            second.add(&taken);
        }
        total.add(&second);
        seconds.push(second);
    };
    let whole = run.as_secs().max(1) as usize;
    for s in 1..=whole {
        let due = started + Duration::from_secs(s as u64);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
        if s == whole {
            stop.store(true, Ordering::Relaxed);
        }
        take(&mut seconds, &mut total);
        each(seconds.last().unwrap(), s);
    }
    for w in workers {
        let _ = w.join();
    }
    let elapsed = started.elapsed().min(run.max(Duration::from_secs(1)));
    // What the connections finished as they stopped: their last answers, counted in the last
    // second.
    let mut late = Seen::new();
    for w in &windows {
        late.add(&w.lock().unwrap());
    }
    total.add(&late);
    if let Some(last) = seconds.last_mut() {
        last.add(&late);
    }
    Loaded { seconds, total, elapsed }
}

/// One connection's loop: a request, its answer read whole, again, until `stop`. A connection
/// that fails is opened again (after a moment, so a refused one doesn't spin).
fn connection(addr: SocketAddr, request: &[u8], stop: &AtomicBool, window: &Mutex<Seen>) {
    let mut open: Option<(TcpStream, BufReader<TcpStream>)> = None;
    while !stop.load(Ordering::Relaxed) {
        if open.is_none() {
            match connect(addr) {
                Ok(c) => open = Some(c),
                Err(e) => {
                    failed(window, format!("can't connect: {e}"));
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
            }
        }
        let (stream, reader) = open.as_mut().unwrap();
        let started = Instant::now();
        let answered = stream.write_all(request).map_err(|e| format!("sending: {e}")).and_then(|()| read_answer(reader));
        let us = started.elapsed().as_micros() as u64;
        match answered {
            Ok((status, keep)) => {
                let mut w = window.lock().unwrap();
                w.hist.record(us);
                if !(200..300).contains(&status) {
                    *w.statuses.entry(status).or_default() += 1;
                }
                drop(w);
                if !keep {
                    open = None;
                }
            }
            Err(why) => {
                // Stopping can cut a request short; that isn't the flow's failure.
                if !stop.load(Ordering::Relaxed) {
                    failed(window, why);
                }
                open = None;
            }
        }
    }
}

fn failed(window: &Mutex<Seen>, why: String) {
    let mut w = window.lock().unwrap();
    w.failed += 1;
    w.last_failure = Some(why);
}

fn connect(addr: SocketAddr) -> std::io::Result<(TcpStream, BufReader<TcpStream>)> {
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let reader = BufReader::with_capacity(16 * 1024, stream.try_clone()?);
    Ok((stream, reader))
}

/// Read one answer whole: its status, and whether the connection stays open after it.
fn read_answer(reader: &mut impl BufRead) -> Result<(u16, bool), String> {
    let mut line = String::new();
    next_line(reader, &mut line)?;
    let mut parts = line.split_whitespace();
    let version = parts.next().unwrap_or_default().to_string();
    let status = parts.next().and_then(|s| s.parse::<u16>().ok()).ok_or_else(|| format!("not an HTTP answer: {:?}", line.trim()))?;
    let (mut length, mut chunked, mut keep) = (None, false, version == "HTTP/1.1");
    loop {
        next_line(reader, &mut line)?;
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else { continue };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = value.parse::<u64>().ok();
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        } else if name.eq_ignore_ascii_case("connection") {
            let value = value.to_ascii_lowercase();
            keep = if value.contains("close") { false } else { keep || value.contains("keep-alive") };
        }
    }
    if chunked {
        loop {
            line.clear();
            reader.read_line(&mut line).map_err(|e| format!("reading: {e}"))?;
            let size = u64::from_str_radix(line.trim().split(';').next().unwrap_or("").trim(), 16).map_err(|_| "a chunk without its size".to_string())?;
            skip(reader, size)?;
            line.clear();
            reader.read_line(&mut line).map_err(|e| format!("reading: {e}"))?;
            if size == 0 {
                // Trailers, if any, until the empty line (the one just read, when there are none).
                while !line.trim_end().is_empty() {
                    line.clear();
                    if reader.read_line(&mut line).map_err(|e| format!("reading: {e}"))? == 0 {
                        break;
                    }
                }
                break;
            }
        }
    } else if let Some(n) = length {
        skip(reader, n)?;
    } else if status >= 200 && status != 204 && status != 304 {
        // No length: the body runs until the connection closes.
        std::io::copy(reader, &mut std::io::sink()).map_err(|e| format!("reading: {e}"))?;
        keep = false;
    }
    Ok((status, keep))
}

/// The next line into `line`, or why there is none: the connection closed before it.
fn next_line(reader: &mut impl BufRead, line: &mut String) -> Result<(), String> {
    line.clear();
    match reader.read_line(line) {
        Ok(0) => Err("the connection closed".into()),
        Ok(_) => Ok(()),
        Err(e) => Err(format!("reading: {e}")),
    }
}

fn skip(reader: &mut impl BufRead, n: u64) -> Result<(), String> {
    let got = std::io::copy(&mut reader.take(n), &mut std::io::sink()).map_err(|e| format!("reading: {e}"))?;
    if got < n {
        return Err("the connection closed mid-answer".into());
    }
    Ok(())
}

/// What the server says of a deployment's runs: the seq its next will get, how many times its
/// log was synced, and how many runs of one route it counted unrecorded.
struct Counts {
    next: u64,
    syncs: u64,
    counted: u64,
}

impl Counts {
    fn read(args: &Args, log_path: &str, label: &str) -> Result<Counts, Stop> {
        let log = cmdline::json(WHO, args, "GET", log_path, None)?;
        Ok(Counts { next: log["next"].as_u64().unwrap_or(0), syncs: log["syncs"].as_u64().unwrap_or(0), counted: log["tallies"][label]["runs"].as_u64().unwrap_or(0) })
    }

    fn since(&self, before: &Counts) -> Counts {
        Counts { next: self.next.saturating_sub(before.next), syncs: self.syncs.saturating_sub(before.syncs), counted: self.counted.saturating_sub(before.counted) }
    }
}

/// A route loaded, and what the server did with its runs meanwhile.
struct Outcome {
    route: Route,
    loaded: Loaded,
    /// Its `next` as the runs recorded, its `syncs`, its `counted`.
    server: Counts,
}

impl Outcome {
    fn per_second(&self) -> f64 {
        self.loaded.total.hist.n as f64 / self.loaded.elapsed.as_secs_f64().max(0.001)
    }

    fn json(&self) -> Value {
        let t = &self.loaded.total;
        json!({
            "method": self.route.method,
            "path": self.route.declared,
            "sent": self.route.path,
            "log": self.route.log,
            "requests": t.hist.n,
            "ok": t.ok(),
            "per_second": (self.per_second() * 10.0).round() / 10.0,
            "latency_ms": latency_json(&t.hist),
            "errors": {
                "statuses": t.statuses.iter().map(|(s, n)| (s.to_string(), json!(n))).collect::<serde_json::Map<_, _>>(),
                "failed": t.failed,
                "last_failure": t.last_failure,
            },
            "server": {
                "recorded": self.server.next,
                "syncs": self.server.syncs,
                "counted": self.server.counted,
            },
            "each_second": self.loaded.seconds.iter().map(|s| json!({
                "requests": s.hist.n,
                "errors": s.errors(),
                "p50_ms": ms_value(s.hist.quantile(0.5)),
                "p99_ms": ms_value(s.hist.quantile(0.99)),
            })).collect::<Vec<_>>(),
        })
    }
}

fn latency_json(h: &Hist) -> Value {
    json!({
        "p50": ms_value(h.quantile(0.5)),
        "p90": ms_value(h.quantile(0.9)),
        "p99": ms_value(h.quantile(0.99)),
        "max": ms_value(h.max_us),
        "mean": ms_value(h.sum_us.checked_div(h.n).unwrap_or(0)),
    })
}

/// Microseconds as milliseconds, to the microsecond.
fn ms_value(us: u64) -> f64 {
    us as f64 / 1000.0
}

/// Microseconds as a person reads them: `0.42 ms`, `12.3 ms`, `1,204 ms`.
fn ms(us: u64) -> String {
    let ms = us as f64 / 1000.0;
    if ms < 10.0 {
        format!("{ms:.2} ms")
    } else if ms < 100.0 {
        format!("{ms:.1} ms")
    } else {
        format!("{} ms", grouped(ms.round() as u64))
    }
}

/// `1234567` as `1,234,567`.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What one second saw, as it ends: `GET /strict (strict)  3/10 s  12,345/s  p50 0.41 ms  p99
/// 1.20 ms`, its route named `width` wide, so the routes' lines line up.
fn second_line(route: &Route, width: usize, s: &Seen, of: u64, second: usize) -> String {
    let errors = match s.errors() {
        0 => String::new(),
        n => format!("  {} error{}", grouped(n), if n == 1 { "" } else { "s" }),
    };
    format!(
        "  {:<width$}  {second:>w$}/{of} s  {:>9}/s  p50 {:>8}  p99 {:>8}{errors}",
        route.named(),
        grouped(s.hist.n),
        ms(s.hist.quantile(0.5)),
        ms(s.hist.quantile(0.99)),
        w = of.to_string().len()
    )
}

/// The routes side by side, then what went wrong, if anything did.
fn summary(results: &[Outcome]) -> Vec<String> {
    let mut rows = vec![["route", "log", "req/s", "p50", "p90", "p99", "max", "errors", "recorded", "syncs", "counted"].map(String::from).to_vec()];
    for r in results {
        let t = &r.loaded.total;
        rows.push(vec![
            r.route.label(),
            r.route.log.clone(),
            grouped(r.per_second().round() as u64),
            ms(t.hist.quantile(0.5)),
            ms(t.hist.quantile(0.9)),
            ms(t.hist.quantile(0.99)),
            ms(t.hist.max_us),
            grouped(t.errors()),
            grouped(r.server.next),
            grouped(r.server.syncs),
            grouped(r.server.counted),
        ]);
    }
    let mut lines = cmdline::table(&rows);
    lines.push(String::new());
    lines.push("recorded: runs written to its log; syncs: times its log was synced to disk for them; counted: runs only counted (unrecorded).".into());
    for r in results {
        let t = &r.loaded.total;
        let mut said: Vec<String> = t.statuses.iter().map(|(s, n)| format!("{} answered {s}", grouped(*n))).collect();
        if t.failed > 0 {
            said.push(format!("{} had no answer ({})", grouped(t.failed), t.last_failure.as_deref().unwrap_or("")));
        }
        if !said.is_empty() {
            lines.push(format!("{}: {}", r.route.label(), said.join("; ")));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared() -> Vec<(String, String, String)> {
        [("GET", "/strict", "strict"), ("GET", "/items/:id", "lazy"), ("POST", "/items", "strict")].iter().map(|(m, p, l)| (m.to_string(), p.to_string(), l.to_string())).collect()
    }

    #[test]
    fn its_routes_are_every_get_or_those_named() {
        let all = pick_routes(&declared(), &[]).ok().unwrap();
        assert_eq!(all.iter().map(|r| (r.label(), r.path.as_str())).collect::<Vec<_>>(), [("GET /strict".into(), "/strict"), ("GET /items/:id".into(), "/items/1")]);
        let mixed: Vec<_> = [("GET", "/u", "unrecorded"), ("GET", "/l", "lazy"), ("GET", "/s", "strict")].iter().map(|(m, p, l)| (m.to_string(), p.to_string(), l.to_string())).collect();
        assert_eq!(pick_routes(&mixed, &[]).ok().unwrap().iter().map(|r| r.declared.as_str()).collect::<Vec<_>>(), ["/s", "/l", "/u"]);
        let named = pick_routes(&declared(), &["post /items", "GET /items/42"]).ok().unwrap();
        assert_eq!(named.iter().map(|r| (r.label(), r.path.as_str(), r.log.as_str())).collect::<Vec<_>>(), [("POST /items".into(), "/items", "strict"), ("GET /items/:id".into(), "/items/42", "lazy")]);
        assert!(matches!(pick_routes(&declared(), &["GET /nope"]), Err(Stop::Failed(_))));
        assert!(matches!(pick_routes(&declared(), &["/strict"]), Err(Stop::Mistake(_))));
        assert!(matches!(pick_routes(&[], &[]), Err(Stop::Failed(_))));
        assert!(matches("/files/*rest", "/files/a/b") && !matches("/a/:b", "/a/b/c") && matches("/a/:b", "/a/x"));
    }

    #[test]
    fn a_flow_answers_on_its_servers_host_or_where_to_says() {
        assert_eq!(host_of("http://127.0.0.1:7420"), "127.0.0.1");
        assert_eq!(host_of("https://holon.example.com"), "holon.example.com");
        assert_eq!(host_of("http://[::1]:7420/x"), "::1");
        assert_eq!(Base::parse("http://10.0.0.2:8171/").unwrap(), Base { host: "10.0.0.2".into(), port: 8171 });
        assert_eq!(Base::parse("[::1]:80").unwrap().authority(), "[::1]:80");
        assert!(Base::parse("https://x:443").is_err() && Base::parse("http://x").is_err());
    }

    #[test]
    fn the_histogram_keeps_each_latency_within_two_percent() {
        for us in [0u64, 1, 63, 64, 65, 127, 128, 1000, 4321, 99_999, 1_000_000, 123_456_789] {
            let v = Hist::value(Hist::index(us));
            assert!(v.abs_diff(us) as f64 <= us as f64 * 0.016 + 0.5, "{us} became {v}");
        }
        let mut h = Hist::new();
        for us in 1..=1000 {
            h.record(us);
        }
        let near = |got: u64, want: u64| got.abs_diff(want) as f64 <= want as f64 * 0.02;
        assert!(near(h.quantile(0.5), 500) && near(h.quantile(0.99), 990), "{} {}", h.quantile(0.5), h.quantile(0.99));
        assert_eq!(h.quantile(1.0), 1000);
        let mut two = Hist::new();
        two.add(&h);
        two.add(&h);
        assert_eq!((two.n, two.max_us), (2000, 1000));
        assert!(near(two.quantile(0.5), 500));
    }

    #[test]
    fn an_answer_is_read_whole_by_its_length_or_its_chunks() {
        let two = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nokHTTP/1.1 404 Not Found\r\ntransfer-encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\nHTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 0\r\n\r\n";
        let mut r = BufReader::new(&two[..]);
        assert_eq!(read_answer(&mut r), Ok((200, true)));
        assert_eq!(read_answer(&mut r), Ok((404, true)));
        assert_eq!(read_answer(&mut r), Ok((200, false)));
        assert!(read_answer(&mut r).is_err());
        let cut = b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nok";
        assert!(read_answer(&mut BufReader::new(&cut[..])).is_err());
    }

    #[test]
    fn requests_name_their_host_and_carry_a_body_only_past_get() {
        let base = Base { host: "127.0.0.1".into(), port: 8171 };
        let get = Route { method: "GET".into(), declared: "/x".into(), path: "/x".into(), log: "strict".into() };
        assert_eq!(String::from_utf8(request_bytes(&get, &base, Some("{\"a\":1}"))).unwrap(), "GET /x HTTP/1.1\r\nHost: 127.0.0.1:8171\r\nUser-Agent: holon-load\r\nAccept: */*\r\n\r\n");
        let post = Route { method: "POST".into(), ..get };
        assert!(String::from_utf8(request_bytes(&post, &base, Some("{\"a\":1}"))).unwrap().ends_with("Content-Length: 7\r\n\r\n{\"a\":1}"));
        assert_eq!(grouped(1234567), "1,234,567");
        assert_eq!(ms(420), "0.42 ms");
        assert_eq!(ms(1_204_000), "1,204 ms");
    }

    /// Against a listener of its own: every request answered and timed, a refused port counted
    /// as failures, not answers.
    #[test]
    fn it_loads_a_server_and_counts_what_it_answers() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut stream = stream;
                    let mut n = 0u64;
                    loop {
                        let mut line = String::new();
                        loop {
                            line.clear();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                                return;
                            }
                            if line == "\r\n" {
                                break;
                            }
                        }
                        n += 1;
                        let answer: &[u8] = if n.is_multiple_of(10) { b"HTTP/1.1 500 X\r\ncontent-length: 1\r\n\r\nx" } else { b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok" };
                        if stream.write_all(answer).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        let base = Base { host: "127.0.0.1".into(), port: addr.port() };
        let route = Route { method: "GET".into(), declared: "/".into(), path: "/".into(), log: "strict".into() };
        let mut ticks = 0;
        let loaded = load(addr, &request_bytes(&route, &base, None), 2, Duration::from_secs(1), |_, _| ticks += 1);
        assert_eq!(ticks, 1);
        let t = &loaded.total;
        assert!(t.hist.n > 100, "{}", t.hist.n);
        assert_eq!(t.failed, 0);
        let fivehundreds = t.statuses.get(&500).copied().unwrap_or(0);
        assert!(fivehundreds > 0 && fivehundreds <= t.hist.n / 10 + 2, "{fivehundreds} of {}", t.hist.n);
        assert_eq!(loaded.seconds.iter().map(|s| s.hist.n).sum::<u64>(), t.hist.n);

        // Nothing listening: failures, no answers.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let loaded = load(closed, b"GET / HTTP/1.1\r\n\r\n", 1, Duration::from_secs(1), |_, _| {});
        assert_eq!(loaded.total.hist.n, 0);
        assert!(loaded.total.failed > 0 && loaded.total.last_failure.as_deref().unwrap().starts_with("can't connect"));
    }
}
