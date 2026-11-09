//! Feeds (host-capabilities L4 §5.9.2): what a flow listens to with `on.feed(prices, (e) => …)`,
//! through a feed an admin binds.
//!
//! - **Bound by an admin,** as a mail is (`--feeds`, owner-only; `/admin/feeds`; `holon feed`): a
//!   name a flow imports (`import prices from "feed:prices"`) bound to a feed of server-sent
//!   events, its URL read from a source (`env:…`, `vault:…`) each time a connection opens and never
//!   kept (sse.rs), with the headers each request sends, their values read from sources too; how
//!   many events may wait for their run (`--buffer`), and how long the feed may say nothing before
//!   the connection is opened again (`--idle`). For the whole server or one deployment.
//! - **Checked when bound:** a request to it answers 200 with `text/event-stream`, then it is
//!   closed. The binding keeps the sources, and where the URL points without its login or query,
//!   which is what is shown.
//! - **The grant** names the feed (`FeedEvent("prices")`); a deploy is refused while it isn't bound
//!   ([`Feeds::refuses`]). Several deployments may listen to one feed: each has its own
//!   connection.
//! - **What each connection is doing** (subscriptions.rs keeps it here) is in
//!   `GET /deployments/<slug>/feeds`.

use std::sync::RwLock;
use std::time::Duration;

use rouille::{Request, Response};

use crate::http::{err, read_body};
use crate::permissions::Manifest;
use crate::secret_sources::{Reference, Sources};

use crate::*;

/// How many events wait for their run, unless the binding says.
pub(crate) const BUFFER: usize = 1000;

/// The most events a binding lets wait.
const BUFFER_MOST: usize = 100_000;

/// How long a feed may say nothing, unless the binding says, in seconds.
pub(crate) const IDLE: u64 = 90;

/// The least idle time a binding sets.
const IDLE_LEAST: u64 = 5;

/// One bound feed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Binding {
    /// Where the host reads the feed's URL from (`env:…`, `vault:…`), never the URL itself.
    pub(crate) url: String,
    /// Where that URL points, without its login or query (`https://stream.example.com/v1/prices`).
    pub(crate) address: String,
    /// The headers each request sends: a name, and where its value is read from.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) headers: BTreeMap<String, String>,
    /// How many events may wait for their run before the connection waits too.
    pub(crate) buffer: usize,
    /// How long the feed may say nothing before the connection is opened again, in seconds.
    pub(crate) idle: u64,
}

/// What binding a feed asks for, as `/admin/feeds` takes it.
#[derive(Deserialize, Default, Debug)]
pub(crate) struct Asked {
    #[serde(default)]
    pub(crate) url: Option<String>,
    /// Each header's name, and where its value is read from.
    #[serde(default)]
    pub(crate) headers: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) buffer: Option<u64>,
    /// `90s`, `2m`.
    #[serde(default)]
    pub(crate) idle: Option<String>,
}

/// Every binding, as [`folders::Bound`]: the server's by name, each deployment's under
/// `@deployments`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Bound {
    #[serde(rename = "@deployments", default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) deployments: BTreeMap<String, BTreeMap<String, Binding>>,
    #[serde(flatten)]
    pub(crate) server: BTreeMap<String, Binding>,
}

/// What one deployment's connection to one feed is doing.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct Live {
    /// Where the feed points, as its binding shows it.
    pub(crate) address: String,
    /// `connecting`, `connected` or `reconnecting`.
    pub(crate) status: &'static str,
    /// Since when, in milliseconds since the epoch.
    pub(crate) since: u64,
    /// Why it isn't connected, when it isn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// The id of the last event run on, when the feed gives ids.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) position: Option<String>,
    /// How many events wait for their run.
    pub(crate) waiting: usize,
}

/// The feeds bound on this server, and what each connection to them is doing.
#[derive(Clone)]
pub(crate) struct Feeds(Arc<Inner>);

struct Inner {
    file: Option<PathBuf>,
    bound: RwLock<Bound>,
    /// What a binding's URL and headers are read from: the server's environment and dotenv
    /// file, Vault.
    sources: Sources,
    /// Each connection's state, by deployment and feed.
    live: Mutex<BTreeMap<(String, String), Live>>,
}

impl std::fmt::Debug for Feeds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Feeds").field("bound", &self.0.bound.read().unwrap()).finish()
    }
}

fn given(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn is_source(s: &str) -> bool {
    s.starts_with("env:") || s.starts_with("vault:")
}

impl Feeds {
    /// None, kept nowhere: the tests, and a server told `--feeds :memory:`.
    pub(crate) fn memory() -> Feeds {
        Feeds::with(None, Bound::default())
    }

    fn with(file: Option<PathBuf>, bound: Bound) -> Feeds {
        Feeds(Arc::new(Inner { file, bound: RwLock::new(bound), sources: Sources::none(), live: Mutex::new(BTreeMap::new()) }))
    }

    /// The feeds bound in `file` (none yet if it doesn't exist).
    pub(crate) fn open(file: &str) -> Result<Feeds, String> {
        if file == ":memory:" {
            return Ok(Feeds::memory());
        }
        let path = PathBuf::from(file);
        let bound = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{file} isn't a feeds file: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Bound::default(),
            Err(e) => return Err(format!("can't read {file}: {e}")),
        };
        Ok(Feeds::with(Some(path), bound))
    }

    /// These, reading a binding's URL and headers from `sources`.
    pub(crate) fn with_sources(self, sources: Sources) -> Feeds {
        let inner = Arc::into_inner(self.0).expect("set up before it is shared");
        Feeds(Arc::new(Inner { sources, ..inner }))
    }

    /// Every binding: the server's, and each deployment's.
    pub(crate) fn all(&self) -> Bound {
        self.0.bound.read().unwrap().clone()
    }

    /// The binding deployment `deployment` sees as `name`: its own, else the server's.
    pub(crate) fn resolve(&self, deployment: &str, name: &str) -> Option<Binding> {
        let bound = self.0.bound.read().unwrap();
        bound.deployments.get(deployment).and_then(|d| d.get(name)).or_else(|| bound.server.get(name)).cloned()
    }

    /// Bind `name`, for the whole server or one deployment, replacing any binding of it there
    /// before, as `asked` says: once a request to it, with its headers, answers as a feed does.
    pub(crate) fn bind(&self, name: &str, asked: &Asked, for_deployment: Option<&str>) -> Result<Binding, String> {
        if !folders::is_folder_name(name) || name.contains('-') {
            return Err(format!("{name:?} isn't a feed's name: letters, digits and _, up to 64, as a flow imports it"));
        }
        if let Some(slug) = for_deployment.filter(|s| !folders::is_folder_name(s)) {
            return Err(format!("{slug:?} isn't a deployment's name"));
        }
        let url = given(&asked.url).ok_or("a feed is read from the URL --url names: it names none")?;
        if !is_source(url) {
            return Err("a feed's URL is read from a source, env:HOLON_SECRET_… or vault:…, holding https://host/path".into());
        }
        for (header, source) in &asked.headers {
            if !sse::is_header_name(header) {
                return Err(format!("{header:?} isn't a header's name"));
            }
            if !is_source(source.trim()) {
                return Err(format!("the header {header}'s value is read from a source, env:HOLON_SECRET_… or vault:…"));
            }
        }
        let buffer = match asked.buffer {
            None => BUFFER,
            Some(n) if (1..=BUFFER_MOST as u64).contains(&n) => n as usize,
            Some(_) => return Err(format!("--buffer is how many events may wait: 1 to {BUFFER_MOST}")),
        };
        let idle = match given(&asked.idle) {
            None => IDLE,
            Some(t) => folders::seconds_of(t).ok_or_else(|| format!("{t:?} isn't how long: 90s, 2m"))?,
        };
        if idle < IDLE_LEAST {
            return Err(format!("--idle is {IDLE_LEAST}s at least: a feed that says nothing for less may only be quiet"));
        }
        let headers: BTreeMap<String, String> = asked.headers.iter().map(|(h, s)| (h.clone(), s.trim().to_string())).collect();
        let binding = Binding { url: url.to_string(), address: String::new(), headers, buffer, idle };
        let (parsed, sent) = self.request(&binding)?;
        let binding = Binding { address: parsed.address(), ..binding };
        sse::open(&parsed, &sent, None, Duration::from_secs(10)).map_err(|why| format!("the feed wasn't read: {why}"))?;
        let mut bound = self.0.bound.write().unwrap();
        let mut next = bound.clone();
        match for_deployment {
            Some(slug) => next.deployments.entry(slug.to_string()).or_default().insert(name.to_string(), binding.clone()),
            None => next.server.insert(name.to_string(), binding.clone()),
        };
        self.save(&next)?;
        *bound = next;
        Ok(binding)
    }

    /// `binding`'s URL and headers, read from their sources now.
    fn request(&self, binding: &Binding) -> Result<(sse::Url, sse::Headers), String> {
        let url = sse::Url::parse(&self.0.sources.fetch(&Reference::parse(&binding.url)?)?)?;
        let mut headers = Vec::new();
        for (name, source) in &binding.headers {
            let value = self.0.sources.fetch(&Reference::parse(source)?).map_err(|e| format!("the header {name}: {e}"))?;
            headers.push((name.clone(), value));
        }
        Ok((url, headers))
    }

    /// A connection to `binding`'s feed, from `last_id` when the host knows where it is.
    pub(crate) fn connect(&self, binding: &Binding, last_id: Option<&str>) -> Result<sse::Connection, String> {
        let (url, headers) = self.request(binding)?;
        sse::open(&url, &headers, last_id, Duration::from_secs(binding.idle))
    }

    /// Unbind `name`, the server's or one deployment's: whether it was bound. A connection to it
    /// is closed within a second.
    pub(crate) fn unbind(&self, name: &str, for_deployment: Option<&str>) -> Result<bool, String> {
        let mut bound = self.0.bound.write().unwrap();
        let mut next = bound.clone();
        let removed = match for_deployment {
            Some(slug) => {
                let removed = next.deployments.get_mut(slug).and_then(|d| d.remove(name)).is_some();
                if next.deployments.get(slug).is_some_and(BTreeMap::is_empty) {
                    next.deployments.remove(slug);
                }
                removed
            }
            None => next.server.remove(name).is_some(),
        };
        if !removed {
            return Ok(false);
        }
        self.save(&next)?;
        *bound = next;
        Ok(true)
    }

    fn save(&self, bound: &Bound) -> Result<(), String> {
        let Some(file) = &self.0.file else { return Ok(()) };
        let bytes = serde_json::to_vec_pretty(bound).map_err(|e| e.to_string())?;
        crate::sealing::write_private(file, &bytes).map_err(|e| format!("can't write {}: {e}", file.display()))
    }

    /// Why a deployment asking for `manifest` can't run here, if it can't: a feed it listens to
    /// that isn't bound.
    pub(crate) fn refuses(&self, deployment: &str, manifest: &Manifest) -> Option<String> {
        manifest.capabilities.iter().filter(|c| !manifest.requirements.optional.contains(*c)).find_map(|item| {
            let name = item.strip_prefix("FeedEvent(\"").and_then(|r| r.strip_suffix("\")"))?;
            self.resolve(deployment, name).is_none().then(|| {
                format!("the feed {name:?} isn't bound on this server: an admin binds it (holon feed add {name} --url env:…, or --for {deployment} for this deployment only)")
            })
        })
    }

    // ── what each connection is doing ──

    /// Set what `slug`'s connection to `feed` is doing.
    pub(crate) fn set_live(&self, slug: &str, feed: &str, change: impl FnOnce(&mut Live)) {
        let mut live = self.0.live.lock().unwrap();
        let l = live.entry((slug.to_string(), feed.to_string())).or_insert_with(|| Live { address: String::new(), status: "connecting", since: now_ms(), error: None, position: None, waiting: 0 });
        change(l);
    }

    /// Forget `slug`'s connection to `feed`: it is closed.
    pub(crate) fn forget_live(&self, slug: &str, feed: &str) {
        self.0.live.lock().unwrap().remove(&(slug.to_string(), feed.to_string()));
    }

    /// What `slug`'s connection to `feed` is doing, if it has one.
    pub(crate) fn live(&self, slug: &str, feed: &str) -> Option<Live> {
        self.0.live.lock().unwrap().get(&(slug.to_string(), feed.to_string())).cloned()
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// `GET /deployments/<slug>/feeds`: each feed its handlers listen to, and what its connection is
/// doing.
pub(crate) fn handle_get_feeds(state: &AppState, slug: &str) -> Response {
    let listened: Vec<(String, String)> = match state.deployments.lock().unwrap().get(slug) {
        Some(d) => d.server.as_ref().map(|s| s.feeds.iter().map(|f| (f.feed.clone(), f.label.clone())).collect()).unwrap_or_default(),
        None => return err(404, format!("no deployment {slug:?}")),
    };
    let feeds = &state.folders.feeds;
    let shown: Vec<serde_json::Value> = listened
        .into_iter()
        .map(|(feed, label)| {
            let mut shown = serde_json::json!({ "feed": feed, "label": label, "status": "closed", "waiting": 0 });
            if let Some(address) = feeds.resolve(slug, &feed).map(|b| b.address) {
                shown["address"] = serde_json::json!(address);
            }
            if let Some(live) = feeds.live(slug, &feed) {
                let mut l = serde_json::to_value(&live).unwrap_or_default();
                l["feed"] = serde_json::json!(feed);
                l["label"] = serde_json::json!(label);
                shown = l;
            }
            shown
        })
        .collect();
    Response::json(&serde_json::json!({ "feeds": shown }))
}

// ── admin ──

fn shown(name: &str, deployment: Option<&str>, b: &Binding) -> serde_json::Value {
    serde_json::json!({ "name": name, "deployment": deployment, "url": b.url, "address": b.address, "headers": b.headers, "buffer": b.buffer, "idle": b.idle })
}

fn labelled(name: &str, deployment: Option<&str>) -> String {
    deployment.map_or_else(|| name.to_string(), |d| format!("{name} (for {d})"))
}

/// `/admin/feeds` (list, bind), `/admin/feeds/<name>` (unbind; `?deployment=<slug>` for a
/// deployment's own), or None for any other path. Only an admin gets this far (access.rs); what
/// they do goes in the activity log.
pub(crate) fn route(state: &AppState, request: &Request, by: &str) -> Option<Response> {
    let url = request.url();
    let method = request.method();
    let all = &state.folders.feeds;
    if url == "/admin/feeds" {
        return Some(match method {
            "GET" => {
                let bound = all.all();
                let server = bound.server.iter().map(|(n, b)| shown(n, None, b));
                let own = bound.deployments.iter().flat_map(|(s, bs)| bs.iter().map(move |(n, b)| shown(n, Some(s), b)));
                Response::json(&server.chain(own).collect::<Vec<_>>())
            }
            "POST" => {
                #[derive(Deserialize)]
                struct Bind {
                    name: String,
                    #[serde(flatten)]
                    asked: Asked,
                    #[serde(default)]
                    deployment: Option<String>,
                }
                let bound = read_body(request)
                    .and_then(|b| serde_json::from_slice::<Bind>(&b).map_err(|e| err(400, format!("invalid body: {e}"))))
                    .map(|b| Bind { deployment: b.deployment.filter(|d| !d.trim().is_empty()), ..b })
                    .and_then(|b| {
                        let f = all.bind(&b.name, &b.asked, b.deployment.as_deref()).map_err(|e| err(400, e))?;
                        Ok((b, f))
                    });
                match bound {
                    Ok((b, f)) => {
                        state.activity.record("bound the feed", &labelled(&b.name, b.deployment.as_deref()), Some(by));
                        if let Some(slug) = &b.deployment {
                            let detail = crate::events::Detail { name: Some(b.name.clone()), ..Default::default() };
                            crate::events::note(&state.deployments, slug, "binding", format!("bound the feed {}", b.name), Some(by), detail);
                        }
                        Response::json(&shown(&b.name, b.deployment.as_deref(), &f))
                    }
                    Err(e) => e,
                }
            }
            _ => Response::empty_404(),
        });
    }
    let name = url.strip_prefix("/admin/feeds/")?;
    if method != "DELETE" {
        return Some(Response::empty_404());
    }
    let deployment = request.get_param("deployment").filter(|d| !d.is_empty());
    Some(match all.unbind(name, deployment.as_deref()) {
        Ok(true) => {
            state.activity.record("unbound the feed", &labelled(name, deployment.as_deref()), Some(by));
            if let Some(slug) = &deployment {
                let detail = crate::events::Detail { name: Some(name.to_string()), ..Default::default() };
                crate::events::note(&state.deployments, slug, "binding", format!("unbound the feed {name}"), Some(by), detail);
            }
            Response::empty_204()
        }
        Ok(false) => err(404, format!("no feed {name:?} is bound{}", deployment.map_or(String::new(), |d| format!(" for {d}")))),
        Err(e) => err(500, e),
    })
}

/// `holon feed add NAME --url SOURCE [--header NAME=SOURCE]… [--buffer N] [--idle T] [--for SLUG]
/// | list | remove NAME`: bind, show and unbind the feeds of the server this user runs here (or
/// `--server`), as an admin.
pub(crate) fn cli(args: &[String]) -> i32 {
    let mut server = None;
    let mut for_deployment: Option<String> = None;
    let mut asked = serde_json::Map::new();
    let mut headers = serde_json::Map::new();
    let mut words = Vec::new();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server = args.next().cloned(),
            "--for" => for_deployment = args.next().cloned(),
            "--url" | "--buffer" | "--idle" | "--header" => {
                let Some(v) = args.next() else {
                    eprintln!("holon feed: {a} takes a value\n\n{FEED_USAGE}");
                    return 2;
                };
                match a.as_str() {
                    "--header" => {
                        let Some((name, source)) = v.split_once('=') else {
                            eprintln!("holon feed: --header is NAME=SOURCE (Authorization=env:HOLON_SECRET_PRICES_AUTH)\n\n{FEED_USAGE}");
                            return 2;
                        };
                        headers.insert(name.trim().to_string(), serde_json::Value::String(source.trim().to_string()));
                    }
                    "--buffer" => {
                        let Ok(n) = v.parse::<u64>() else {
                            eprintln!("holon feed: --buffer is how many events may wait: a number\n\n{FEED_USAGE}");
                            return 2;
                        };
                        asked.insert("buffer".into(), serde_json::json!(n));
                    }
                    _ => {
                        asked.insert(a[2..].to_string(), serde_json::Value::String(v.clone()));
                    }
                }
            }
            "--help" | "-h" => {
                println!("{FEED_USAGE}");
                return 0;
            }
            _ if !a.starts_with('-') => words.push(a.as_str()),
            _ => {
                eprintln!("holon feed: no option {a:?}\n\n{FEED_USAGE}");
                return 2;
            }
        }
    }
    let (method, path, body) = match words.as_slice() {
        ["list"] => ("GET", "/admin/feeds".to_string(), None),
        ["add", name] => {
            if !asked.contains_key("url") {
                eprintln!("holon feed add: it takes --url SOURCE, where the server reads the feed's URL\n\n{FEED_USAGE}");
                return 2;
            }
            asked.insert("name".into(), serde_json::Value::String(name.to_string()));
            asked.insert("headers".into(), serde_json::Value::Object(headers));
            asked.insert("deployment".into(), serde_json::json!(for_deployment));
            ("POST", "/admin/feeds".to_string(), Some(serde_json::Value::Object(asked)))
        }
        ["remove", name] => {
            let only = for_deployment.as_ref().map_or(String::new(), |d| format!("?deployment={}", crate::mcp::seg(d)));
            ("DELETE", format!("/admin/feeds/{}{only}", crate::mcp::seg(name)), None)
        }
        _ => {
            eprintln!("{FEED_USAGE}");
            return 2;
        }
    };
    let (server, answer) = match local::admin_call("holon feed", server.as_deref(), method, &path, body.as_ref()) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let line = |f: &serde_json::Value| {
        let only = f["deployment"].as_str().map_or(String::new(), |s| format!(", for {s} only"));
        let headers: Vec<String> = f["headers"].as_object().into_iter().flatten().map(|(h, s)| format!("{h} from {}", s.as_str().unwrap_or(""))).collect();
        let mut said = vec![f["name"].as_str().unwrap_or("").to_string(), format!("reads {} (from {})", f["address"].as_str().unwrap_or(""), f["url"].as_str().unwrap_or(""))];
        if !headers.is_empty() {
            said.push(format!("sends {}", headers.join(", ")));
        }
        said.push(format!("{} may wait  quiet for {} at most", f["buffer"].as_u64().unwrap_or(BUFFER as u64), folders::span_text(f["idle"].as_u64().unwrap_or(IDLE))));
        format!("{}{only}", said.join("  "))
    };
    match words[0] {
        "list" => {
            let all: Vec<serde_json::Value> = serde_json::from_str(&answer).unwrap_or_default();
            if all.is_empty() {
                println!("no feeds are bound on {server}: holon feed add NAME --url env:… binds one");
            }
            for f in &all {
                println!("{}", line(f));
            }
        }
        "add" => println!("bound {}", line(&serde_json::from_str(&answer).unwrap_or_default())),
        _ => println!("unbound {}{}: its connections close", words[1], for_deployment.as_ref().map_or(String::new(), |d| format!(" (for {d})"))),
    }
    0
}

const FEED_USAGE: &str = "usage: holon feed add NAME --url SOURCE [--header NAME=SOURCE]… [--buffer N] [--idle T] [--for SLUG]
       holon feed list
       holon feed remove NAME [--for SLUG]
       (each with [--server PORT|URL])

  Binds a name a flow imports (import prices from \"feed:prices\") to a feed of server-sent
  events, whose events start its runs (on.feed(prices, (e) => …)), one at a time, in the order
  they came. A flow never sees the feed's URL or its headers. Admins.

  add NAME                 bind NAME, once a request to it answers as a feed does
    --url SOURCE           where the server reads the feed's URL, as it reads a secret:
                           env:HOLON_SECRET_PRICES_URL (its environment or --dotenv file) or
                           vault:<mount>/<path>#<field>. The URL is never stored, shown or
                           logged: https://stream.example.com/v1/prices?key=…, a login in it
                           sent as Basic authentication; plain http:// only to this machine
    --header NAME=SOURCE   a header each request sends, its value read from a source:
                           Authorization=env:HOLON_SECRET_PRICES_AUTH (again for another)
    --buffer N             how many events may wait for their run before the connection waits
                           too (1000)
    --idle T               how long the feed may say nothing before the connection is opened
                           again: 90s, 2m (5s at least)
    --for SLUG             bind it for that deployment only
  list                     the names bound, and where each reads
  remove NAME              unbind it (--for SLUG: that deployment's); its connections close
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon feed add prices --url env:HOLON_SECRET_PRICES_URL
  holon feed add prices --url env:HOLON_SECRET_PRICES_URL --header Authorization=env:HOLON_SECRET_PRICES_AUTH --idle 2m
  holon feed list

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  with an API token in HOLON_TOKEN.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sse::stand_in::StandIn;

    fn feeds(vars: &[(&str, &str)]) -> Feeds {
        Feeds::memory().with_sources(Sources::of(vars))
    }

    fn asked(url: &str) -> Asked {
        Asked { url: Some(url.into()), ..Asked::default() }
    }

    #[test]
    fn a_feed_is_bound_once_it_answers_as_a_feed_and_shown_without_its_query() {
        let feed = StandIn::start();
        let url = feed.url("/events?key=s3cret");
        let all = feeds(&[("HOLON_SECRET_PRICES_URL", &url), ("HOLON_SECRET_PRICES_AUTH", "Bearer t0ken")]);
        let mut a = asked("env:HOLON_SECRET_PRICES_URL");
        a.headers.insert("Authorization".into(), "env:HOLON_SECRET_PRICES_AUTH".into());
        a.idle = Some("2m".into());
        let b = all.bind("prices", &a, None).unwrap();
        assert_eq!(b.address, url.split('?').next().unwrap());
        assert_eq!((b.buffer, b.idle), (BUFFER, 120));
        assert_eq!(b.headers["Authorization"], "env:HOLON_SECRET_PRICES_AUTH");
        assert!(feed.asked()[0].contains("Authorization: Bearer t0ken"));
        assert_eq!(all.resolve("app", "prices"), Some(b.clone()));
        // A deployment's own is its own.
        all.bind("prices", &Asked { buffer: Some(5), ..asked("env:HOLON_SECRET_PRICES_URL") }, Some("app")).unwrap();
        assert_eq!(all.resolve("app", "prices").unwrap().buffer, 5);
        assert_eq!(all.resolve("other", "prices"), Some(b));
        assert!(all.unbind("prices", Some("app")).unwrap());
        assert!(!all.unbind("prices", Some("app")).unwrap());
    }

    #[test]
    fn a_binding_is_refused_for_what_isnt_a_feed() {
        let feed = StandIn::start();
        let all = feeds(&[
            ("HOLON_SECRET_PLAIN", &feed.url("/plain")),
            ("HOLON_SECRET_GONE", &feed.url("/gone")),
            ("HOLON_SECRET_REMOTE", "http://stream.example.com/e?key=s3cret"),
            ("HOLON_SECRET_OK", &feed.url("/events")),
        ]);
        let refused = |a: &Asked| all.bind("prices", a, None).unwrap_err();
        assert!(refused(&asked("env:HOLON_SECRET_PLAIN")).ends_with("answered with text/plain, not text/event-stream"));
        assert!(refused(&asked("env:HOLON_SECRET_GONE")).ends_with("/gone answered 404"));
        let remote = refused(&asked("env:HOLON_SECRET_REMOTE"));
        assert!(remote.contains("plain http:// only to this machine") && !remote.contains("s3cret"), "{remote}");
        assert!(refused(&asked("https://stream.example.com/e")).starts_with("a feed's URL is read from a source"));
        assert!(refused(&asked("env:HOLON_SECRET_MISSING")).contains("HOLON_SECRET_MISSING"));
        let mut a = asked("env:HOLON_SECRET_OK");
        a.headers.insert("Authorization".into(), "env:HOLON_SECRET_NOT_THERE".into());
        assert!(refused(&a).starts_with("the header Authorization: "), "{}", refused(&a));
        a.headers = BTreeMap::from([("Authorization".into(), "Bearer x".into())]);
        assert!(refused(&a).contains("is read from a source"));
        assert!(refused(&Asked { idle: Some("1s".into()), ..asked("env:HOLON_SECRET_OK") }).starts_with("--idle is 5s at least"));
        assert!(refused(&Asked { buffer: Some(0), ..asked("env:HOLON_SECRET_OK") }).starts_with("--buffer is how many events may wait"));
        assert!(all.bind("pri-ces", &asked("env:HOLON_SECRET_OK"), None).unwrap_err().contains("isn't a feed's name"));
        assert_eq!(all.all(), Bound::default());
    }

    #[test]
    fn a_deploy_listening_to_a_feed_no_one_bound_is_refused() {
        let feed = StandIn::start();
        let all = feeds(&[("HOLON_SECRET_PRICES_URL", &feed.url("/events"))]);
        let manifest = Manifest { capabilities: ["FeedEvent(\"prices\")".to_string()].into_iter().collect(), requirements: Default::default() };
        assert!(all.refuses("app", &manifest).unwrap().starts_with("the feed \"prices\" isn't bound on this server"));
        all.bind("prices", &asked("env:HOLON_SECRET_PRICES_URL"), Some("app")).unwrap();
        assert_eq!(all.refuses("app", &manifest), None);
        assert!(all.refuses("other", &manifest).is_some());
    }
}
