//! A deployable unit's manifest as consent names it: what it asks the host for. One source for
//! the rules both sides apply: holon-server grants and enforces with them (permissions.rs), and
//! the editor shows and asks with them, through holon-wasm, never a TypeScript copy that could
//! drift from what the server decides.
//!
//! - A capability's **consent name**: `NetworkIngress`, `StateRead("todos")`,
//!   `HttpRequest("hooks.slack.com")` (its `Debug` form, the name a deploy acknowledges).
//! - The manifest's **items**: each capability, then each limit the author asks for
//!   (`Limit(request_timeout_ms=60000)`), each host called over plain HTTP (`PlainHttp("h")`), and
//!   `PrivateAddresses`; sorted, as a set.
//! - Its **trigger**: what starts its runs, never optional.
//! - Each item's **kind**: required (missing, the deployment pauses) or optional (the author
//!   marked it so; missing, it runs with those calls refused).
//! - Whether a unit **reads from outside** a run's inputs: then it can't be recomputed offline from
//!   them alone, and a replay shows what the run recorded it produced.

use std::collections::{BTreeMap, BTreeSet};

use crate::Capability;

/// How consent names `c`.
pub fn consent_name(c: &Capability) -> String {
    format!("{c:?}")
}

/// The manifest's items: `capabilities` (consent names), then what the author asks beyond the
/// host's defaults.
pub fn manifest_items(capabilities: &[String], limits: &BTreeMap<String, u64>, plain_http: &BTreeSet<String>, private_addresses: bool) -> BTreeSet<String> {
    let mut items: BTreeSet<String> = capabilities.iter().cloned().collect();
    for (name, n) in limits {
        items.insert(format!("Limit({name}={n})"));
    }
    for host in plain_http {
        items.insert(format!("PlainHttp({host:?})"));
    }
    if private_addresses {
        items.insert("PrivateAddresses".into());
    }
    items
}

/// What starts the runs of a unit with `capabilities`: `NetworkIngress` for one with routes, else
/// `Clock` for one the clock fires; none for one only triggered by hand.
pub fn trigger(capabilities: &[String]) -> Option<&'static str> {
    let has = |c: &str| capabilities.iter().any(|x| x == c);
    if has("NetworkIngress") {
        Some("NetworkIngress")
    } else if has("Clock") {
        Some("Clock")
    } else {
        None
    }
}

/// Whether a manifest item is required or optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionKind {
    /// Missing it, the deployment pauses.
    Required,
    /// The author marked it so: missing it, the deployment runs with those calls refused.
    Optional,
}

/// `item`'s kind, given what the author marked `optional` and the unit's `trigger`: optional only
/// when marked so, and its trigger never is.
pub fn permission_kind(item: &str, optional: &BTreeSet<String>, trigger: Option<&str>) -> PermissionKind {
    if Some(item) != trigger && optional.contains(item) {
        PermissionKind::Optional
    } else {
        PermissionKind::Required
    }
}

/// Whether a unit with these capabilities (consent names) reads from outside a run's inputs: the
/// clock, randomness, a request, a secret, state, a collection, the network, a waiting point.
/// Effects it only performs (writes, sends) don't count: they come out as values.
pub fn reads_from_outside(capabilities: &[String]) -> bool {
    const SOURCED: &[&str] = &["Clock", "Random", "NetworkIngress", "SecretRead", "StateRead", "CollectionRead", "HttpRequest", "Await"];
    capabilities.iter().any(|c| SOURCED.contains(&c.split('(').next().unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn consent_names_what_a_deploy_acknowledges() {
        assert_eq!(consent_name(&Capability::NetworkIngress), "NetworkIngress");
        assert_eq!(consent_name(&Capability::StateRead("todos".into())), "StateRead(\"todos\")");
        assert_eq!(consent_name(&Capability::HttpRequest("hooks.slack.com".into())), "HttpRequest(\"hooks.slack.com\")");
    }

    #[test]
    fn items_are_the_capabilities_then_what_the_author_asks_sorted() {
        let items = manifest_items(
            &caps(&["SecretRead(\"k\")", "NetworkIngress"]),
            &BTreeMap::from([("max_requests".to_string(), 80)]),
            &BTreeSet::from(["a.local:8080".to_string()]),
            true,
        );
        assert_eq!(
            items.into_iter().collect::<Vec<_>>(),
            ["Limit(max_requests=80)", "NetworkIngress", "PlainHttp(\"a.local:8080\")", "PrivateAddresses", "SecretRead(\"k\")"]
        );
    }

    #[test]
    fn the_trigger_is_required_and_only_what_is_marked_is_optional() {
        let server = caps(&["Clock", "NetworkIngress", "SecretRead(\"k\")"]);
        assert_eq!(trigger(&server), Some("NetworkIngress"));
        assert_eq!(trigger(&caps(&["Clock", "StateWrite(\"b\")"])), Some("Clock"));
        assert_eq!(trigger(&caps(&["StateWrite(\"b\")"])), None);
        let optional = BTreeSet::from(["SecretRead(\"k\")".to_string(), "NetworkIngress".to_string()]);
        let t = trigger(&server);
        assert_eq!(permission_kind("SecretRead(\"k\")", &optional, t), PermissionKind::Optional);
        assert_eq!(permission_kind("Clock", &optional, t), PermissionKind::Required);
        assert_eq!(permission_kind("NetworkIngress", &optional, t), PermissionKind::Required, "a trigger even when marked");
    }

    #[test]
    fn reading_from_outside_is_sourcing_not_performing() {
        assert!(reads_from_outside(&caps(&["SecretRead(\"k\")"])));
        assert!(reads_from_outside(&caps(&["Clock"])));
        assert!(reads_from_outside(&caps(&["CollectionRead(\"c\")"])));
        assert!(!reads_from_outside(&caps(&["StateWrite(\"k\")", "CollectionWrite(\"c\")", "NetworkEgress"])));
        assert!(!reads_from_outside(&[]));
    }
}
