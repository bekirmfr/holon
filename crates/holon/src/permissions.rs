//! Manifests and permissions: the flow asks, the host grants (§4.2.1 of docs/PLATFORM_LAYOUT.md).
//!
//! - A **manifest** is what a deployment needs: the capabilities derived from its graph (exact,
//!   they can't be understated) plus the requirements its author declares (limits above the host
//!   defaults, exceptions to the outbound rules). It is content-addressed: its hash changes exactly
//!   when what the flow needs changes.
//! - The **policy** is the host operator's (`holon.toml`): each limit's default and ceiling, what is
//!   granted without asking, and which exceptions may ever be granted.
//! - A **grant** is what the host gave one deployment, persisted with it. Deploy is a request: what
//!   policy allows is granted, what needs approval must be acknowledged, what policy forbids is
//!   refused. A redeploy asks only for what the previous grant didn't cover.
//! - A **permit** is a grant applied to the policy in force: the limits, outbound hosts and
//!   exceptions every run of the deployment is held to. Effects outside it are refused at perform
//!   time, whatever the graph says.
//! - The host can **change a grant** at any time, without a redeploy: revoke an item, grant back
//!   one the manifest asks for, lower a limit. A grant that no longer covers its manifest pauses
//!   the deployment until it does again, unless the host chose to keep it running, revoked calls
//!   failing. Every change is recorded on the grant.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use holon_core::{hash_value, Capability, Hash, Value};
use holon_host::Limits;
use serde::{Deserialize, Serialize};

use crate::channels::ChannelLimits;
use crate::outbound::Outbound;
use crate::to_hex;

/// The limits a deployment may ask for, by name, and the unit each is counted in. Durations are
/// milliseconds (the durable-run wait is in seconds, like the `await` node's `expires`); sizes are
/// bytes. The two module limits are what a flow's module bodies may compute and hold per call
/// (`docs/L4s/wasm-module-L4.md` §4): derived from the units' own budgets, never a default a run
/// gets, so the policy has ceilings for them and no `[limits]` entry.
pub(crate) const LIMIT_NAMES: [&str; 29] = [
    "connect_timeout_ms",
    "request_timeout_ms",
    "claim_wait_ms",
    "commit_wait_ms",
    "run_budget_ms",
    "clock_run_budget_ms",
    "max_requests",
    "per_host_concurrency",
    "max_request_bytes",
    "max_response_bytes",
    "max_header_bytes",
    "max_redirects",
    "max_await_seconds",
    "max_waiting_runs",
    "inline_file_bytes",
    "max_file_read_bytes",
    "max_file_write_bytes",
    "max_files_per_run",
    "max_db_rows",
    "max_db_read_bytes",
    "db_statement_ms",
    "max_db_statements",
    "max_pack_bytes",
    "max_packs_per_run",
    "channel_keep",
    "channel_buffer",
    "max_subscribers",
    holon_core::FUEL_LIMIT,
    holon_core::MEMORY_LIMIT,
];

/// What no module may be granted past: a hundred times the default fuel, sixteen times the
/// default memory. A flow asking more is refused at deploy, like any limit over its ceiling.
pub(crate) const MODULE_FUEL_CEILING: u64 = 100 * holon_core::DEFAULT_MODULE_FUEL;
pub(crate) const MODULE_MEMORY_CEILING: u64 = 16 * holon_core::DEFAULT_MODULE_MEMORY;

/// Limits on durable runs (§4.3): how long one waiting point may wait, and how many runs may wait
/// at once. The host's, like the request limits; not part of a run (`holon_host::Limits`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DurableLimits {
    pub(crate) max_await_seconds: u64,
    pub(crate) max_waiting_runs: u64,
}

impl DurableLimits {
    /// What a deployment gets without asking: a month's wait (a 7-day `await` fits), and as many
    /// waiting runs as before this was a limit.
    pub(crate) fn defaults() -> DurableLimits {
        DurableLimits { max_await_seconds: 30 * 24 * 3600, max_waiting_runs: 10_000 }
    }

    /// What no grant can exceed: core's longest wait, and ten times the default number.
    pub(crate) fn ceilings() -> DurableLimits {
        DurableLimits { max_await_seconds: holon_core::MAX_AWAIT_SECONDS, max_waiting_runs: 100_000 }
    }
}

/// Limits on a run's files (docs/L4s/named-folders-L4.md §9): the host's, like the durable-run
/// limits; not part of a run (`holon_host::Limits`). Sizes in bytes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FileLimits {
    /// The largest file a run reads whole as text, and the largest range it reads of one.
    pub(crate) inline_file_bytes: u64,
    /// What a run reads from files, in all.
    pub(crate) max_file_read_bytes: u64,
    /// What a run writes and copies into files, in all.
    pub(crate) max_file_write_bytes: u64,
    /// The files a run reads, takes a handle to, writes, copies or deletes.
    pub(crate) max_files_per_run: u64,
}

impl FileLimits {
    /// What a deployment gets without asking: a text up to 1 MiB, 64 MiB read; a gigabyte written,
    /// since a copy by handle is how a big file moves; a thousand files.
    pub(crate) fn defaults() -> FileLimits {
        FileLimits { inline_file_bytes: 1 << 20, max_file_read_bytes: 64 << 20, max_file_write_bytes: 1 << 30, max_files_per_run: 1000 }
    }

    /// What no grant can exceed.
    pub(crate) fn ceilings() -> FileLimits {
        FileLimits { inline_file_bytes: 16 << 20, max_file_read_bytes: 1 << 30, max_file_write_bytes: 1 << 40, max_files_per_run: 100_000 }
    }
}

/// Limits on a run's database reads and statements (`docs/L4s/databases-L4.md` §5): the host's,
/// like the file limits. A read past one fails the run; it is never cut short.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DbLimits {
    /// The rows a run's queries give, in all.
    pub(crate) max_db_rows: u64,
    /// The bytes of what they give, in all.
    pub(crate) max_db_read_bytes: u64,
    /// How long one statement may take.
    pub(crate) db_statement: Duration,
    /// The statements a run sends: its queries and its writes.
    pub(crate) max_db_statements: u64,
}

impl DbLimits {
    /// What a deployment gets without asking: ten thousand rows, 16 MiB, five seconds a
    /// statement, a hundred statements.
    pub(crate) fn defaults() -> DbLimits {
        DbLimits { max_db_rows: 10_000, max_db_read_bytes: 16 << 20, db_statement: Duration::from_secs(5), max_db_statements: 100 }
    }

    /// What no grant can exceed.
    pub(crate) fn ceilings() -> DbLimits {
        DbLimits { max_db_rows: 1_000_000, max_db_read_bytes: 256 << 20, db_statement: Duration::from_secs(60), max_db_statements: 10_000 }
    }
}

/// `numbers` applied over `base`'s database limits.
fn numbers_to_databases(base: &DbLimits, numbers: &BTreeMap<String, u64>) -> DbLimits {
    let n = |k: &str, d: u64| numbers.get(k).copied().unwrap_or(d);
    DbLimits {
        max_db_rows: n("max_db_rows", base.max_db_rows),
        max_db_read_bytes: n("max_db_read_bytes", base.max_db_read_bytes),
        db_statement: Duration::from_millis(n("db_statement_ms", base.db_statement.as_millis() as u64)),
        max_db_statements: n("max_db_statements", base.max_db_statements),
    }
}

/// The limits as numbers by name: the request-run limits, the clock run's budget, the
/// durable-run limits, the file limits, the database limits and the channel limits.
fn limits_to_numbers(request: &Limits, clock_budget: Duration, durable: &DurableLimits, files: &FileLimits, databases: &DbLimits, channels: &ChannelLimits) -> BTreeMap<String, u64> {
    let ms = |d: Duration| d.as_millis() as u64;
    BTreeMap::from([
        ("connect_timeout_ms".into(), ms(request.connect_timeout)),
        ("request_timeout_ms".into(), ms(request.request_timeout)),
        ("claim_wait_ms".into(), ms(request.claim_wait)),
        ("commit_wait_ms".into(), ms(request.commit_wait)),
        ("run_budget_ms".into(), ms(request.run_budget)),
        ("clock_run_budget_ms".into(), ms(clock_budget)),
        ("max_requests".into(), request.max_requests as u64),
        ("per_host_concurrency".into(), request.per_host_concurrency as u64),
        ("max_request_bytes".into(), request.max_request_bytes as u64),
        ("max_response_bytes".into(), request.max_response_bytes as u64),
        ("max_header_bytes".into(), request.max_header_bytes as u64),
        ("max_redirects".into(), request.max_redirects as u64),
        ("max_await_seconds".into(), durable.max_await_seconds),
        ("max_waiting_runs".into(), durable.max_waiting_runs),
        ("inline_file_bytes".into(), files.inline_file_bytes),
        ("max_file_read_bytes".into(), files.max_file_read_bytes),
        ("max_file_write_bytes".into(), files.max_file_write_bytes),
        ("max_files_per_run".into(), files.max_files_per_run),
        ("max_db_rows".into(), databases.max_db_rows),
        ("max_db_read_bytes".into(), databases.max_db_read_bytes),
        ("db_statement_ms".into(), ms(databases.db_statement)),
        ("max_db_statements".into(), databases.max_db_statements),
        ("max_pack_bytes".into(), channels.max_pack_bytes),
        ("max_packs_per_run".into(), channels.max_packs_per_run),
        ("channel_keep".into(), channels.channel_keep),
        ("channel_buffer".into(), channels.channel_buffer),
        ("max_subscribers".into(), channels.max_subscribers),
    ])
}

/// `numbers` applied over `base`'s channel limits.
fn numbers_to_channels(base: &ChannelLimits, numbers: &BTreeMap<String, u64>) -> ChannelLimits {
    let n = |k: &str, d: u64| numbers.get(k).copied().unwrap_or(d);
    ChannelLimits {
        max_pack_bytes: n("max_pack_bytes", base.max_pack_bytes),
        max_packs_per_run: n("max_packs_per_run", base.max_packs_per_run),
        channel_keep: n("channel_keep", base.channel_keep),
        channel_buffer: n("channel_buffer", base.channel_buffer),
        max_subscribers: n("max_subscribers", base.max_subscribers),
    }
}

/// `numbers` applied over `base`'s file limits.
fn numbers_to_files(base: &FileLimits, numbers: &BTreeMap<String, u64>) -> FileLimits {
    let n = |k: &str, d: u64| numbers.get(k).copied().unwrap_or(d);
    FileLimits {
        inline_file_bytes: n("inline_file_bytes", base.inline_file_bytes),
        max_file_read_bytes: n("max_file_read_bytes", base.max_file_read_bytes),
        max_file_write_bytes: n("max_file_write_bytes", base.max_file_write_bytes),
        max_files_per_run: n("max_files_per_run", base.max_files_per_run),
    }
}

/// `numbers` applied over `base`'s durable-run limits.
fn numbers_to_durable(base: &DurableLimits, numbers: &BTreeMap<String, u64>) -> DurableLimits {
    DurableLimits {
        max_await_seconds: numbers.get("max_await_seconds").copied().unwrap_or(base.max_await_seconds),
        max_waiting_runs: numbers.get("max_waiting_runs").copied().unwrap_or(base.max_waiting_runs),
    }
}

/// `numbers` applied over `base`, for a request run, or (`clock`) a clock run.
fn numbers_to_limits(base: &Limits, numbers: &BTreeMap<String, u64>, clock: bool) -> Limits {
    let mut l = base.clone();
    for (name, &n) in numbers {
        match name.as_str() {
            "connect_timeout_ms" => l.connect_timeout = Duration::from_millis(n),
            "request_timeout_ms" => l.request_timeout = Duration::from_millis(n),
            "claim_wait_ms" => l.claim_wait = Duration::from_millis(n),
            "commit_wait_ms" => l.commit_wait = Duration::from_millis(n),
            "run_budget_ms" if !clock => l.run_budget = Duration::from_millis(n),
            "clock_run_budget_ms" if clock => l.run_budget = Duration::from_millis(n),
            "max_requests" => l.max_requests = n as usize,
            "per_host_concurrency" => l.per_host_concurrency = n as usize,
            "max_request_bytes" => l.max_request_bytes = n as usize,
            "max_response_bytes" => l.max_response_bytes = n as usize,
            "max_header_bytes" => l.max_header_bytes = n as usize,
            "max_redirects" => l.max_redirects = n as u32,
            _ => {}
        }
    }
    l
}

/// What an author declares a flow needs beyond what its graph shows.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Requirements {
    /// Limits it needs set to something other than the host default, by name (`LIMIT_NAMES`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) limits: BTreeMap<String, u64>,
    /// Hosts it calls over plain HTTP: an exception to "HTTPS only".
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) plain_http: BTreeSet<String>,
    /// Whether it calls private addresses: an exception to "no private destinations".
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) private_addresses: bool,
    /// Capabilities it can run without, in the author's word (as Chrome's `optional_permissions`
    /// or Android's `required="false"`): one not granted, or revoked, leaves it running with
    /// those calls refused, where a required one pauses it. Everything else is required, and its
    /// trigger ([`Manifest::trigger`]) is never optional.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) optional: BTreeSet<String>,
}

impl Requirements {
    /// These requirements with the budgets of the modules `root` places (`module_limits_of`):
    /// a flow with a module asks for `module_fuel` and `module_memory` at the largest its
    /// modules declare, whether or not its author wrote them, so the person approving sees how
    /// much it may compute per call and the policy's ceilings hold it.
    pub(crate) fn with_module_limits(mut self, root: &holon_core::Unit, resolver: &impl holon_core::Resolver) -> Requirements {
        for (name, n) in holon_core::module_limits_of(root, resolver) {
            let asked = self.limits.entry(name).or_insert(0);
            *asked = (*asked).max(n);
        }
        self
    }
}

/// What a deployment needs: its derived capabilities and its declared requirements.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Manifest {
    /// Each capability as consent names it (`StateRead("todos")`), sorted.
    pub(crate) capabilities: Vec<String>,
    pub(crate) requirements: Requirements,
}

impl Manifest {
    pub(crate) fn new(caps: impl Iterator<Item = Capability>, requirements: Requirements) -> Manifest {
        let mut capabilities: Vec<String> = caps.map(|c| holon_core::consent_name(&c)).collect();
        capabilities.sort();
        Manifest { capabilities, requirements }
    }

    /// Everything in it that can be granted, as consent names it (core's `manifest_items`, the
    /// rule the editor shows with too): each capability, each limit asked for
    /// (`Limit(request_timeout_ms=60000)`), each exception (`PlainHttp("a.com")`,
    /// `PrivateAddresses`).
    pub(crate) fn items(&self) -> BTreeSet<String> {
        let r = &self.requirements;
        holon_core::manifest_items(&self.capabilities, &r.limits, &r.plain_http, r.private_addresses)
    }

    /// The manifest's content hash: equal manifests, equal hashes. Which items are optional is
    /// part of it once any are (so hashes from before optional items still match).
    pub(crate) fn hash(&self) -> Hash {
        let items = Value::List(self.items().into_iter().map(Value::Text).collect());
        if self.requirements.optional.is_empty() {
            return hash_value(&items).expect("a manifest is small text");
        }
        let optional = Value::List(self.requirements.optional.iter().cloned().map(Value::Text).collect());
        let record = Value::Record(BTreeMap::from([("items".to_string(), items), ("optional".to_string(), optional)]));
        hash_value(&record).expect("a manifest is small text")
    }

    /// What starts its runs, which it can't do without (core's `trigger`).
    pub(crate) fn trigger(&self) -> Option<&str> {
        holon_core::trigger(&self.capabilities)
    }

    /// Whether it can run without `item` (core's `permission_kind`): the author marked it
    /// optional, and it isn't the trigger.
    pub(crate) fn is_optional(&self, item: &str) -> bool {
        holon_core::permission_kind(item, &self.requirements.optional, self.trigger()) == holon_core::PermissionKind::Optional
    }

    /// Why its optional items don't hold, if they don't: one it doesn't need, or its trigger.
    pub(crate) fn optional_problem(&self) -> Option<String> {
        self.requirements.optional.iter().find_map(|item| {
            if Some(item.as_str()) == self.trigger() {
                Some(format!("{item} is what starts its runs, so it can't be optional"))
            } else if !self.capabilities.contains(item) {
                Some(format!("{item} is marked optional, but the flow doesn't use it"))
            } else {
                None
            }
        })
    }
}

/// The host operator's policy. Read from `holon.toml` at startup; built-in defaults without one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Policy {
    /// The limits a request run gets without asking.
    pub(crate) request: Limits,
    /// The run budget a clock run gets without asking (its other limits are `request`'s).
    pub(crate) clock_run_budget: Duration,
    /// The durable-run limits a deployment gets without asking.
    pub(crate) durable: DurableLimits,
    /// The file limits a deployment gets without asking.
    pub(crate) files: FileLimits,
    /// The database limits a deployment gets without asking.
    pub(crate) databases: DbLimits,
    /// The channel limits a deployment gets without asking.
    pub(crate) channels: ChannelLimits,
    /// What no grant can exceed.
    pub(crate) ceilings: BTreeMap<String, u64>,
    /// Items granted without asking: exact (`HttpRequest("hooks.slack.com")`) or a whole kind
    /// (`StateRead(*)`).
    pub(crate) auto: Vec<String>,
    /// Whether a deployment may ever be granted plain HTTP, or private addresses.
    pub(crate) allow_plain_http: bool,
    pub(crate) allow_private_addresses: bool,
}

impl Default for Policy {
    fn default() -> Policy {
        let mut ceilings = limits_to_numbers(&Limits::ceilings(), Limits::ceilings().run_budget, &DurableLimits::ceilings(), &FileLimits::ceilings(), &DbLimits::ceilings(), &ChannelLimits::ceilings());
        ceilings.insert(holon_core::FUEL_LIMIT.into(), MODULE_FUEL_CEILING);
        ceilings.insert(holon_core::MEMORY_LIMIT.into(), MODULE_MEMORY_CEILING);
        Policy {
            request: Limits::request_run(),
            clock_run_budget: Limits::clock_run().run_budget,
            durable: DurableLimits::defaults(),
            files: FileLimits::defaults(),
            databases: DbLimits::defaults(),
            channels: ChannelLimits::defaults(),
            ceilings,
            auto: Vec::new(),
            allow_plain_http: false,
            allow_private_addresses: false,
        }
    }
}

/// `holon.toml`'s shape. Every section and key is optional.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    #[serde(default)]
    limits: BTreeMap<String, u64>,
    #[serde(default)]
    ceilings: BTreeMap<String, u64>,
    #[serde(default)]
    grants: GrantsFile,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct GrantsFile {
    #[serde(default)]
    auto: Vec<String>,
    #[serde(default)]
    allow_plain_http: bool,
    #[serde(default)]
    allow_private_addresses: bool,
}

impl Policy {
    /// The policy in `path`, or the defaults when there is no such file. A file that exists but
    /// doesn't parse, names an unknown limit, or sets a default above its ceiling is an error:
    /// the server refuses to start rather than run under a policy nobody wrote.
    pub(crate) fn load(path: &str) -> Result<Policy, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Policy::default()),
            Err(e) => return Err(format!("could not read {path:?}: {e}")),
        };
        Policy::parse(&text).map_err(|e| format!("{path:?}: {e}"))
    }

    pub(crate) fn parse(text: &str) -> Result<Policy, String> {
        let file: PolicyFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut policy = Policy::default();
        for name in file.limits.keys().chain(file.ceilings.keys()) {
            if !LIMIT_NAMES.contains(&name.as_str()) {
                return Err(format!("unknown limit {name:?} (known: {})", LIMIT_NAMES.join(", ")));
            }
        }
        for name in [holon_core::FUEL_LIMIT, holon_core::MEMORY_LIMIT] {
            if file.limits.contains_key(name) {
                return Err(format!("{name} is a module's own budget, not a default a run gets: set its [ceilings] entry instead"));
            }
        }
        policy.ceilings.extend(file.ceilings);
        let mut defaults = policy.defaults();
        defaults.extend(file.limits);
        for (name, n) in &defaults {
            if n > &policy.ceilings[name] {
                return Err(format!("the default {name} = {n} is above its ceiling {}", policy.ceilings[name]));
            }
        }
        policy.request = numbers_to_limits(&policy.request, &defaults, false);
        policy.clock_run_budget = Duration::from_millis(defaults["clock_run_budget_ms"]);
        policy.durable = numbers_to_durable(&policy.durable, &defaults);
        policy.files = numbers_to_files(&policy.files, &defaults);
        policy.databases = numbers_to_databases(&policy.databases, &defaults);
        policy.channels = numbers_to_channels(&policy.channels, &defaults);
        policy.auto = file.grants.auto;
        policy.allow_plain_http = file.grants.allow_plain_http;
        policy.allow_private_addresses = file.grants.allow_private_addresses;
        Ok(policy)
    }

    /// The defaults, by name.
    pub(crate) fn defaults(&self) -> BTreeMap<String, u64> {
        limits_to_numbers(&self.request, self.clock_run_budget, &self.durable, &self.files, &self.databases, &self.channels)
    }

    /// Whether `item` is granted without asking.
    fn is_auto(&self, item: &str) -> bool {
        self.auto.iter().any(|pattern| match pattern.strip_suffix("(*)") {
            Some(kind) => item.starts_with(&format!("{kind}(")),
            None => pattern == item,
        })
    }

    /// Why policy can't grant `manifest` at all, if it can't: a limit above its ceiling, an
    /// unknown limit, or an exception policy never grants.
    pub(crate) fn refuses(&self, manifest: &Manifest) -> Option<String> {
        manifest.optional_problem().or_else(|| manifest.items().iter().find_map(|item| self.refuses_item(item)))
    }

    /// Why policy can't grant `item`, if it can't.
    pub(crate) fn refuses_item(&self, item: &str) -> Option<String> {
        if let Some((name, n)) = parse_limit(item) {
            return match self.ceilings.get(&name) {
                None => Some(format!("unknown limit {name:?} (known: {})", LIMIT_NAMES.join(", "))),
                Some(ceiling) if n > *ceiling => Some(format!("{name} = {n} is above this host's ceiling of {ceiling}")),
                _ => None,
            };
        }
        // The host by name: it comes from the deploy settings, not the flow.
        if let Some(host) = item.strip_prefix("PlainHttp(").and_then(|h| h.strip_suffix(')'))
            && !self.allow_plain_http
        {
            return Some(format!(
                "this host never grants plain HTTP, asked for {host} (the deploy settings' \"plain HTTP to\"; holon.toml: grants.allow_plain_http)"
            ));
        }
        if item == "PrivateAddresses" && !self.allow_private_addresses {
            return Some("this host never grants private addresses (holon.toml: grants.allow_private_addresses)".into());
        }
        None
    }

    /// The items of `manifest` someone must approve: everything but what policy grants without
    /// asking, what `previous` already granted, and limits at or below the default (lowering a
    /// limit needs nobody's permission).
    pub(crate) fn needs_approval(&self, manifest: &Manifest, previous: Option<&Grant>) -> BTreeSet<String> {
        let defaults = self.defaults();
        let lowered: BTreeSet<String> = manifest
            .requirements
            .limits
            .iter()
            .filter(|(name, n)| defaults.get(*name).is_some_and(|d| *n <= d))
            .map(|(name, n)| format!("Limit({name}={n})"))
            .collect();
        manifest
            .items()
            .into_iter()
            .filter(|item| !self.is_auto(item) && !lowered.contains(item))
            .filter(|item| previous.is_none_or(|g| !g.items.contains(item)))
            .collect()
    }
}

/// `Limit(name=n)` as its name and number.
fn parse_limit(item: &str) -> Option<(String, u64)> {
    let (name, n) = item.strip_prefix("Limit(")?.strip_suffix(')')?.split_once('=')?;
    Some((name.to_string(), n.parse().ok()?))
}

/// What the host granted one deployment. Persisted with it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Grant {
    /// The hash (hex) of the manifest it was granted for.
    pub(crate) manifest: String,
    /// Every item granted: the manifest's, whether approved or granted by policy, less what the
    /// host has revoked since.
    pub(crate) items: BTreeSet<String>,
    /// What the deployment asked for beyond its graph (its manifest's requirements); what of it
    /// is granted is in `items`.
    pub(crate) requirements: Requirements,
    pub(crate) granted_at: u64,
    /// How: `"deploy"` (acknowledged), or `"migrated"` (from a file before grants, whose
    /// capabilities were acknowledged at an earlier deploy).
    pub(crate) by: String,
    /// Items the host took back since: offered back, and an outbound call to a revoked host
    /// fails as `revoked`.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) revoked: BTreeSet<String>,
    /// Limits the host lowered since, by name: each holds runs below what was asked or the
    /// default, whichever applied.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) lowered: BTreeMap<String, u64>,
    /// What a grant that no longer covers the manifest does: pause the deployment until it does
    /// again (`false`), or keep it running, the calls it lacks failing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) keep_running: bool,
    /// Whether the deployment is paused because of this grant (so covering it again resumes it).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) paused: bool,
    /// Every change the host made since the deploy, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) changes: Vec<GrantChange>,
}

/// One change the host made to a grant.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct GrantChange {
    pub(crate) at: u64,
    pub(crate) change: String,
}

/// A change to a grant, as the host asks for it (`POST /deployments/<slug>/grant`). Applied whole
/// or not at all.
#[derive(Deserialize, Default, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChangeRequest {
    /// Granted items to take back.
    #[serde(default)]
    pub(crate) revoke: Vec<String>,
    /// Items the manifest asks for, to grant (back).
    #[serde(default)]
    pub(crate) grant: Vec<String>,
    /// Limits to lower, by name. Raising one takes a deploy, which asks for approval.
    #[serde(default)]
    pub(crate) lower: BTreeMap<String, u64>,
    #[serde(default)]
    pub(crate) keep_running: Option<bool>,
}

impl Grant {
    pub(crate) fn for_manifest(manifest: &Manifest, now: u64, by: &str) -> Grant {
        Grant {
            manifest: to_hex(&manifest.hash()),
            items: manifest.items(),
            requirements: manifest.requirements.clone(),
            granted_at: now,
            by: by.into(),
            revoked: BTreeSet::new(),
            lowered: BTreeMap::new(),
            keep_running: false,
            paused: false,
            changes: Vec::new(),
        }
    }

    /// The manifest this grant was made for, given what the unit's graph derives.
    pub(crate) fn manifest_of(&self, caps: impl Iterator<Item = Capability>) -> Manifest {
        Manifest::new(caps, self.requirements.clone())
    }

    /// This grant with the host's change applied, or why it can't be (nothing changes then).
    pub(crate) fn change(&self, manifest: &Manifest, policy: &Policy, req: &ChangeRequest, now: u64) -> Result<Grant, String> {
        let mut g = self.clone();
        let mut changes = Vec::new();
        for item in &req.revoke {
            if !g.items.remove(item) {
                return Err(format!("{item} isn't granted to this deployment"));
            }
            g.revoked.insert(item.clone());
            changes.push(format!("revoked {item}"));
        }
        let asked = manifest.items();
        for item in &req.grant {
            if !asked.contains(item) {
                return Err(format!("{item} isn't something this deployment asks for"));
            }
            if let Some(why) = policy.refuses_item(item) {
                return Err(format!("this host can't grant {item}: {why}"));
            }
            if let Some((name, _)) = parse_limit(item) {
                g.lowered.remove(&name);
            }
            g.items.insert(item.clone());
            g.revoked.remove(item);
            changes.push(format!("granted {item}"));
        }
        for (name, &n) in &req.lower {
            let Some(current) = g.limit(name, policy) else {
                return Err(format!("unknown limit {name:?} (known: {})", LIMIT_NAMES.join(", ")));
            };
            if n > current {
                return Err(format!(
                    "{name} is {current} for this deployment; raising it takes a deploy, which asks for approval"
                ));
            }
            // A raised limit it asked for above the new value isn't granted any more.
            if let Some(&asked_n) = g.requirements.limits.get(name)
                && asked_n > n
            {
                let item = format!("Limit({name}={asked_n})");
                if g.items.remove(&item) {
                    g.revoked.insert(item);
                }
            }
            g.lowered.insert(name.clone(), n);
            changes.push(format!("lowered {name} to {n}"));
        }
        if let Some(keep) = req.keep_running
            && keep != g.keep_running
        {
            g.keep_running = keep;
            changes.push(if keep { "keeps running when not covered" } else { "pauses when not covered" }.into());
        }
        if changes.is_empty() {
            return Err("nothing to change".into());
        }
        g.changes.extend(changes.into_iter().map(|change| GrantChange { at: now, change }));
        Ok(g)
    }

    /// Limit `name` as this grant holds runs to it, before the policy's ceilings: what it asked
    /// for if that is granted, else the host default, then no more than the host lowered it to.
    fn limit(&self, name: &str, policy: &Policy) -> Option<u64> {
        let default = *policy.defaults().get(name)?;
        let base = self.granted_limits().get(name).copied().unwrap_or(default);
        Some(self.lowered.get(name).map_or(base, |l| base.min(*l)))
    }

    /// The limits it asked for that are granted.
    fn granted_limits(&self) -> BTreeMap<String, u64> {
        self.requirements
            .limits
            .iter()
            .filter(|(name, n)| self.items.contains(&format!("Limit({name}={n})")))
            .map(|(name, n)| (name.clone(), *n))
            .collect()
    }

    /// Why the deployment can't run under this grant and the policy in force, if it can't: it
    /// needs required items it isn't granted (unless it keeps running without them), or the host
    /// no longer grants what it asks for. Optional items it lacks never stop it. Its trigger
    /// always does: the host supplies what starts a run (a request's body, the time), never asking
    /// the permit, so a deployment running without it would go on being started.
    pub(crate) fn attention(&self, manifest: &Manifest, policy: &Policy) -> Option<String> {
        let missing: Vec<String> = self.missing(manifest).into_iter().filter(|i| !manifest.is_optional(i)).collect();
        if let Some(trigger) = manifest.trigger().filter(|t| missing.iter().any(|i| i == t)) {
            return Some(format!("{trigger} starts its runs and isn't granted; grant it back, or deploy it again to approve"));
        }
        if !missing.is_empty() && !self.keep_running {
            return Some(format!(
                "it needs {} it isn't granted; grant it back, or deploy it again to approve",
                missing.join(", ")
            ));
        }
        policy.refuses(manifest).map(|why| format!("this host no longer grants it: {why}"))
    }

    /// What `manifest` needs that this grant doesn't give.
    pub(crate) fn missing(&self, manifest: &Manifest) -> Vec<String> {
        manifest.items().into_iter().filter(|i| !self.items.contains(i)).collect()
    }

    /// The grant's content hash, recorded in every flow-log entry it governed. Covers what it
    /// grants (lowered limits only once there are any, so older hashes still match).
    pub(crate) fn hash(&self) -> Hash {
        let items = Value::List(self.items.iter().cloned().map(Value::Text).collect());
        let mut record = BTreeMap::from([
            ("manifest".to_string(), Value::Text(self.manifest.clone())),
            ("items".to_string(), items),
        ]);
        if !self.lowered.is_empty() {
            let lowered = self.lowered.iter().map(|(k, n)| (k.clone(), Value::Number(*n as f64))).collect();
            record.insert("lowered".to_string(), Value::Record(lowered));
        }
        hash_value(&Value::Record(record)).expect("a grant is small text")
    }
}

/// A grant applied to the policy in force: what every run of the deployment is held to.
#[derive(Debug)]
pub(crate) struct Permit {
    /// The deployment's unit hash: a run id names it.
    pub(crate) deployment: Hash,
    pub(crate) items: BTreeSet<String>,
    pub(crate) request_limits: Limits,
    pub(crate) clock_limits: Limits,
    /// How long a waiting point may wait, and how many runs may wait at once.
    pub(crate) durable: DurableLimits,
    /// How much of its files a run may read and write.
    pub(crate) files: FileLimits,
    /// How much of its databases a run may read, and how many statements it sends.
    pub(crate) databases: DbLimits,
    /// What its runs may send on its channels, and how many may subscribe.
    pub(crate) channels: ChannelLimits,
    pub(crate) outbound: Outbound,
    /// The grant's hash (hex), for the flow log.
    pub(crate) grant: String,
}

impl Permit {
    pub(crate) fn new(deployment: Hash, grant: &Grant, policy: &Policy) -> Permit {
        // The limits it was granted, lowered where the host lowered them; then a ceiling lowered
        // since the grant wins: the operator's word is the last one.
        let mut numbers = grant.granted_limits();
        for name in grant.lowered.keys() {
            if let Some(n) = grant.limit(name, policy) {
                numbers.insert(name.clone(), n);
            }
        }
        let clamped: BTreeMap<String, u64> = numbers
            .iter()
            .map(|(name, n)| (name.clone(), policy.ceilings.get(name).map_or(*n, |c| (*n).min(*c))))
            .collect();
        let request_limits = numbers_to_limits(&policy.request, &clamped, false);
        let clock_base = Limits { run_budget: policy.clock_run_budget, ..policy.request.clone() };
        let clock_limits = numbers_to_limits(&clock_base, &clamped, true);
        let durable = numbers_to_durable(&policy.durable, &clamped);
        let files = numbers_to_files(&policy.files, &clamped);
        let databases = numbers_to_databases(&policy.databases, &clamped);
        let channels = numbers_to_channels(&policy.channels, &clamped);
        let hosts = |items: &BTreeSet<String>| -> BTreeSet<String> {
            items
                .iter()
                .filter_map(|i| i.strip_prefix("HttpRequest(\"")?.strip_suffix("\")").map(str::to_string))
                .collect()
        };
        // An exception holds while it is granted and policy still allows it.
        let plain_http = grant
            .requirements
            .plain_http
            .iter()
            .filter(|h| policy.allow_plain_http && grant.items.contains(&format!("PlainHttp({h:?})")))
            .cloned()
            .collect();
        let outbound = Outbound {
            allowed: hosts(&grant.items),
            revoked: hosts(&grant.revoked),
            plain_http,
            allow_private: grant.requirements.private_addresses
                && grant.items.contains("PrivateAddresses")
                && policy.allow_private_addresses,
        };
        Permit { deployment, items: grant.items.clone(), request_limits, clock_limits, durable, files, databases, channels, outbound, grant: to_hex(&grant.hash()) }
    }

    /// Whether the grant covers this capability.
    pub(crate) fn allows(&self, cap: &Capability) -> bool {
        self.items.contains(&format!("{cap:?}"))
    }

    /// The names inside the granted items of one kind (`CollectionRead("notes")` → `notes`).
    fn named(&self, kind: &str) -> BTreeSet<String> {
        let prefix = format!("{kind}(\"");
        self.items.iter().filter_map(|i| i.strip_prefix(prefix.as_str())?.strip_suffix("\")").map(str::to_string)).collect()
    }

    /// The collections it may read, by name.
    pub(crate) fn collections_read(&self) -> BTreeSet<String> {
        self.named("CollectionRead")
    }

    /// The collections it may write, by name: the ones whose keys a run claims before calling out.
    pub(crate) fn collections_written(&self) -> BTreeSet<String> {
        self.named("CollectionWrite")
    }

    /// The channels it may send on, by name: the ones a piped call may send its answer to.
    pub(crate) fn channels_sent(&self) -> BTreeSet<String> {
        self.named("ChannelSend")
    }

    /// The folders it may read, by name.
    pub(crate) fn folders_read(&self) -> BTreeSet<String> {
        self.named("FileRead")
    }

    /// The folders it may write, by name.
    pub(crate) fn folders_written(&self) -> BTreeSet<String> {
        self.named("FileWrite")
    }

    /// The tables it may read, as `shop/orders`.
    pub(crate) fn tables_read(&self) -> BTreeSet<String> {
        self.named("DbRead")
    }

    /// The tables it may write, as `shop/orders`.
    pub(crate) fn tables_written(&self) -> BTreeSet<String> {
        self.named("DbWrite")
    }

    /// The databases it may read or write, by name.
    pub(crate) fn databases_named(&self) -> BTreeSet<String> {
        let tables = self.named("DbRead").into_iter().chain(self.named("DbWrite"));
        tables.map(|t| t.split('/').next().unwrap_or(&t).to_string()).collect()
    }

    /// The folders whose files it may pass on by handle (copy, or send as a body): the ones it
    /// reads, and the ones whose new files start its runs (that file is handed to the run).
    pub(crate) fn folders_handed(&self) -> BTreeSet<String> {
        let mut folders = self.folders_read();
        folders.extend(self.named("FileArrival"));
        folders
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(caps: &[Capability], requirements: Requirements) -> Manifest {
        Manifest::new(caps.iter().cloned(), requirements)
    }

    #[test]
    fn a_policy_file_sets_defaults_ceilings_and_auto_grants_and_refuses_nonsense() {
        let p = Policy::parse(
            r#"
            [limits]
            request_timeout_ms = 20000
            [ceilings]
            max_requests = 200
            [grants]
            auto = ["StateRead(*)", "HttpRequest(\"hooks.slack.com\")"]
            allow_plain_http = true
            "#,
        )
        .unwrap();
        assert_eq!(p.request.request_timeout, Duration::from_millis(20000));
        assert_eq!(p.ceilings["max_requests"], 200);
        assert_eq!(p.request.max_requests, 50, "untouched defaults stay");
        assert!(p.allow_plain_http && !p.allow_private_addresses);
        assert_eq!(Policy::parse("").unwrap(), Policy::default());
        assert!(Policy::parse("[limits]\nmax_request = 3").unwrap_err().contains("unknown limit"));
        assert!(Policy::parse("[limits]\nmax_requests = 5000").unwrap_err().contains("above its ceiling"));
        assert!(Policy::parse("[grant]\nauto = []").is_err(), "a misspelt section is an error, not ignored");

        // The durable-run limits are limits like the others.
        let p = Policy::parse("[limits]\nmax_waiting_runs = 500\n[ceilings]\nmax_waiting_runs = 1000").unwrap();
        assert_eq!((p.durable.max_waiting_runs, p.ceilings["max_waiting_runs"]), (500, 1000));
        assert_eq!(Policy::default().durable.max_await_seconds, 30 * 24 * 3600);
        assert!(Policy::parse("[ceilings]\nmax_await_seconds = 86400").unwrap_err().contains("above its ceiling"), "the default month");

        // So are the file limits.
        let p = Policy::parse("[limits]\nmax_files_per_run = 50\n[ceilings]\ninline_file_bytes = 4194304").unwrap();
        assert_eq!((p.files.max_files_per_run, p.ceilings["inline_file_bytes"]), (50, 4 << 20));
        assert_eq!(Policy::default().files, FileLimits::defaults());
        assert!(Policy::parse("[limits]\nmax_file_read_bytes = 2147483648").unwrap_err().contains("above its ceiling"), "past a gigabyte");
    }

    /// A flow that places a module asks for its budgets as limits, whether or not its author
    /// wrote them; the policy has ceilings for those and no defaults.
    #[test]
    fn a_module_s_budgets_are_limits_with_ceilings_and_no_defaults() {
        use holon_core::{Edge, MapResolver, Port, Type, UnitBody, BOUNDARY, FUEL_LIMIT, MEMORY_LIMIT};
        let module = holon_core::Unit {
            body: UnitBody::Module { bytes: b"\0asm\x01\0\0\0".to_vec() }, // never run here: only its params are read
            params: vec![holon_core::Param::frozen("fuel", "50000000"), holon_core::Param::frozen("memory", "1048576")],
            inputs: vec![Port { name: "a".into(), ty: Type::Any }],
            outputs: vec![Port { name: "out".into(), ty: Type::Any }],
        };
        let mut store = MapResolver::new();
        let h = store.insert(module).unwrap();
        let root = holon_core::Unit {
            body: UnitBody::Composite {
                children: vec![h],
                edges: vec![Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 }, Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
            },
            params: vec![],
            inputs: vec![Port { name: "a".into(), ty: Type::Any }],
            outputs: vec![Port { name: "out".into(), ty: Type::Any }],
        };
        let mut declared = Requirements::default();
        declared.limits.insert(MEMORY_LIMIT.into(), 4 << 20); // the author asked for more memory than the module declares
        let req = declared.with_module_limits(&root, &store);
        assert_eq!(req.limits[FUEL_LIMIT], 50_000_000);
        assert_eq!(req.limits[MEMORY_LIMIT], 4 << 20, "the larger of the two");
        let manifest = Manifest::new(std::iter::empty(), req);
        assert!(manifest.items().contains("Limit(module_fuel=50000000)"), "{:?}", manifest.items());
        assert_eq!(Policy::default().refuses(&manifest), None);

        // Past the ceiling, refused like any limit; the ceiling is the operator's to raise.
        let mut greedy = Requirements::default();
        greedy.limits.insert(FUEL_LIMIT.into(), MODULE_FUEL_CEILING + 1);
        assert!(Policy::default().refuses(&Manifest::new(std::iter::empty(), greedy.clone())).unwrap().contains("above this host's ceiling"));
        let raised = Policy::parse(&format!("[ceilings]\n{FUEL_LIMIT} = {}", MODULE_FUEL_CEILING * 2)).unwrap();
        assert_eq!(raised.refuses(&Manifest::new(std::iter::empty(), greedy)), None);
        // A default for a module budget makes no sense: a module's budget is its own.
        assert!(Policy::parse(&format!("[limits]\n{FUEL_LIMIT} = 5")).unwrap_err().contains("module's own budget"));
    }

    #[test]
    fn deploy_asks_only_for_what_policy_and_the_previous_grant_dont_cover() {
        let policy = Policy::parse("[grants]\nauto = [\"StateRead(*)\"]").unwrap();
        let caps = [
            Capability::NetworkIngress,
            Capability::StateRead("todos".into()),
            Capability::HttpRequest("api.example.com".into()),
        ];
        let raised = Requirements { limits: BTreeMap::from([("request_timeout_ms".into(), 60_000)]), ..Default::default() };
        let m = manifest(&caps, raised);
        let ask: Vec<String> = policy.needs_approval(&m, None).into_iter().collect();
        assert_eq!(ask, vec!["HttpRequest(\"api.example.com\")", "Limit(request_timeout_ms=60000)", "NetworkIngress"]);

        // A redeploy that adds one host asks for that host alone.
        let granted = Grant::for_manifest(&m, 0, "deploy");
        let more = manifest(&[caps.as_slice(), &[Capability::HttpRequest("b.example.com".into())]].concat(), m.requirements.clone());
        assert_eq!(policy.needs_approval(&more, Some(&granted)).into_iter().collect::<Vec<_>>(), vec!["HttpRequest(\"b.example.com\")"]);
        assert_eq!(granted.missing(&more), vec!["HttpRequest(\"b.example.com\")"]);

        // Lowering a limit needs nobody; above the ceiling, or an exception policy forbids, is refused.
        let lower = Requirements { limits: BTreeMap::from([("max_requests".into(), 5)]), ..Default::default() };
        assert!(policy.needs_approval(&manifest(&[], lower), None).is_empty());
        let huge = Requirements { limits: BTreeMap::from([("max_requests".into(), 5000)]), ..Default::default() };
        assert!(policy.refuses(&manifest(&[], huge)).unwrap().contains("ceiling of 1000"));
        let private = Requirements { private_addresses: true, ..Default::default() };
        assert!(policy.refuses(&manifest(&[], private)).unwrap().contains("private addresses"));
    }

    #[test]
    fn a_permit_applies_the_grant_within_the_policy_in_force() {
        let caps = [Capability::HttpRequest("api.example.com".into()), Capability::StateWrite("k".into())];
        let req = Requirements {
            limits: BTreeMap::from([("request_timeout_ms".into(), 90_000), ("clock_run_budget_ms".into(), 120_000)]),
            plain_http: ["api.example.com".to_string()].into(),
            private_addresses: true,
            ..Requirements::default()
        };
        let grant = Grant::for_manifest(&manifest(&caps, req), 0, "deploy");
        let open = Policy { allow_plain_http: true, allow_private_addresses: true, ..Policy::default() };
        let p = Permit::new([1; 32], &grant, &open);
        assert_eq!(p.request_limits.request_timeout, Duration::from_secs(90));
        assert_eq!(p.request_limits.run_budget, Duration::from_secs(25), "the request run keeps its own budget");
        assert_eq!(p.clock_limits.run_budget, Duration::from_secs(120));
        assert_eq!(p.outbound.allowed, ["api.example.com".to_string()].into());
        assert!(p.outbound.allow_private && p.outbound.plain_http.contains("api.example.com"));
        assert!(p.allows(&Capability::StateWrite("k".into())) && !p.allows(&Capability::StateWrite("other".into())));

        // The operator closed the exceptions and lowered a ceiling since: the permit follows.
        let mut strict = Policy::default();
        strict.ceilings.insert("request_timeout_ms".into(), 45_000);
        let p = Permit::new([1; 32], &grant, &strict);
        assert_eq!(p.request_limits.request_timeout, Duration::from_secs(45));
        assert!(!p.outbound.allow_private && p.outbound.plain_http.is_empty());
    }

    #[test]
    fn an_optional_item_missing_keeps_it_running_and_a_required_one_pauses_it() {
        let policy = Policy::default();
        let caps = [Capability::NetworkIngress, Capability::SecretRead("receipt".into()), Capability::SecretRead("token".into())];
        let optional = |items: &[&str]| Requirements { optional: items.iter().map(|i| i.to_string()).collect(), ..Requirements::default() };
        let m = manifest(&caps, optional(&["SecretRead(\"receipt\")"]));
        assert_eq!(policy.refuses(&m), None);
        let mut grant = Grant::for_manifest(&m, 0, "deploy");
        grant.items.remove("SecretRead(\"receipt\")");
        assert_eq!(grant.attention(&m, &policy), None, "optional: it keeps running, those reads refused");
        grant.items.remove("SecretRead(\"token\")");
        assert!(grant.attention(&m, &policy).unwrap().contains("SecretRead(\"token\")"), "required: it pauses");
        assert!(!grant.attention(&m, &policy).unwrap().contains("receipt"), "the optional one isn't named");
        grant.keep_running = true;
        assert_eq!(grant.attention(&m, &policy), None, "kept running, its reads refused");
        // Its trigger the host supplies without asking the permit: without it, it always pauses.
        grant.items.remove("NetworkIngress");
        assert!(grant.attention(&m, &policy).unwrap().contains("NetworkIngress starts its runs"), "kept running or not");

        // Its trigger can't be optional, nor can something it doesn't use.
        assert!(policy.refuses(&manifest(&caps, optional(&["NetworkIngress"]))).unwrap().contains("starts its runs"));
        assert!(policy.refuses(&manifest(&caps, optional(&["Clock"]))).unwrap().contains("doesn't use it"));
        let clock_flow = manifest(&[Capability::Clock, Capability::StateWrite("beats".into())], optional(&["Clock"]));
        assert!(policy.refuses(&clock_flow).unwrap().contains("starts its runs"));

        // Which items are optional is part of the manifest; without any, hashes are as before.
        assert_ne!(m.hash(), manifest(&caps, Requirements::default()).hash());
        let items = Value::List(manifest(&caps, Requirements::default()).items().into_iter().map(Value::Text).collect());
        assert_eq!(manifest(&caps, Requirements::default()).hash(), hash_value(&items).unwrap());
    }

    #[test]
    fn a_manifests_hash_follows_what_it_needs() {
        let a = manifest(&[Capability::NetworkIngress], Requirements::default());
        let b = manifest(&[Capability::NetworkIngress], Requirements::default());
        assert_eq!(a.hash(), b.hash());
        let c = manifest(&[Capability::NetworkIngress], Requirements { private_addresses: true, ..Default::default() });
        assert_ne!(a.hash(), c.hash());
    }
}
