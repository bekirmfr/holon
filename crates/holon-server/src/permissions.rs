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

use crate::outbound::Outbound;
use crate::to_hex;

/// The limits a deployment may ask for, by name, and the unit each is counted in. Durations are
/// milliseconds (the durable-run wait is in seconds, like the `await` node's `expires`); sizes are
/// bytes.
pub(crate) const LIMIT_NAMES: [&str; 12] = [
    "connect_timeout_ms",
    "request_timeout_ms",
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
];

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

/// The limits as numbers by name: the request-run limits, the clock run's budget, and the
/// durable-run limits.
fn limits_to_numbers(request: &Limits, clock_budget: Duration, durable: &DurableLimits) -> BTreeMap<String, u64> {
    let ms = |d: Duration| d.as_millis() as u64;
    BTreeMap::from([
        ("connect_timeout_ms".into(), ms(request.connect_timeout)),
        ("request_timeout_ms".into(), ms(request.request_timeout)),
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
    ])
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
        let mut capabilities: Vec<String> = caps.map(|c| format!("{c:?}")).collect();
        capabilities.sort();
        Manifest { capabilities, requirements }
    }

    /// Everything in it that can be granted, as consent names it: each capability, each limit
    /// asked for (`Limit(request_timeout_ms=60000)`), each exception (`PlainHttp("a.com")`,
    /// `PrivateAddresses`).
    pub(crate) fn items(&self) -> BTreeSet<String> {
        let mut items: BTreeSet<String> = self.capabilities.iter().cloned().collect();
        for (name, n) in &self.requirements.limits {
            items.insert(format!("Limit({name}={n})"));
        }
        for host in &self.requirements.plain_http {
            items.insert(format!("PlainHttp({host:?})"));
        }
        if self.requirements.private_addresses {
            items.insert("PrivateAddresses".into());
        }
        items
    }

    /// The manifest's content hash: equal manifests, equal hashes.
    pub(crate) fn hash(&self) -> Hash {
        let items = Value::List(self.items().into_iter().map(Value::Text).collect());
        hash_value(&items).expect("a manifest is small text")
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
        Policy {
            request: Limits::request_run(),
            clock_run_budget: Limits::clock_run().run_budget,
            durable: DurableLimits::defaults(),
            ceilings: limits_to_numbers(&Limits::ceilings(), Limits::ceilings().run_budget, &DurableLimits::ceilings()),
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
        policy.auto = file.grants.auto;
        policy.allow_plain_http = file.grants.allow_plain_http;
        policy.allow_private_addresses = file.grants.allow_private_addresses;
        Ok(policy)
    }

    /// The defaults, by name.
    pub(crate) fn defaults(&self) -> BTreeMap<String, u64> {
        limits_to_numbers(&self.request, self.clock_run_budget, &self.durable)
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
        manifest.items().iter().find_map(|item| self.refuses_item(item))
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
        if item.starts_with("PlainHttp(") && !self.allow_plain_http {
            return Some("this host never grants plain HTTP (holon.toml: grants.allow_plain_http)".into());
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
    /// needs items it isn't granted (unless it keeps running without them), or the host no
    /// longer grants what it asks for.
    pub(crate) fn attention(&self, manifest: &Manifest, policy: &Policy) -> Option<String> {
        let missing = self.missing(manifest);
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
        Permit { deployment, items: grant.items.clone(), request_limits, clock_limits, durable, outbound, grant: to_hex(&grant.hash()) }
    }

    /// Whether the grant covers this capability.
    pub(crate) fn allows(&self, cap: &Capability) -> bool {
        self.items.contains(&format!("{cap:?}"))
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
    fn a_manifests_hash_follows_what_it_needs() {
        let a = manifest(&[Capability::NetworkIngress], Requirements::default());
        let b = manifest(&[Capability::NetworkIngress], Requirements::default());
        assert_eq!(a.hash(), b.hash());
        let c = manifest(&[Capability::NetworkIngress], Requirements { private_addresses: true, ..Default::default() });
        assert_ne!(a.hash(), c.hash());
    }
}
