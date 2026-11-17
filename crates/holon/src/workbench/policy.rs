//! What a deploy's review says beside its diff, as the web console's Review says it
//! (editor/src/reviewMay.ts): while it runs (its port, its run budget, the runs it keeps, what it
//! holds between runs), and the lines of the host's policy it relies on, each holding or not.

use serde_json::Value;

/// A number of milliseconds as the web writes it: whole seconds as seconds.
fn ms(n: u64) -> String {
    if n >= 1000 && n.is_multiple_of(1000) { format!("{} s", n / 1000) } else { format!("{n} ms") }
}

/// The touches it keeps (not gone).
fn live(touches: &Value) -> Vec<&Value> {
    touches.as_array().into_iter().flatten().filter(|t| t["change"] != "gone").collect()
}

/// `names` said as "key a, b" or "keys a, b".
fn named(ts: &[&Value], kind: &str, one: &str, many: &str) -> Option<String> {
    let names: Vec<&str> = ts.iter().filter(|t| t["kind"] == kind).filter_map(|t| t["name"].as_str()).collect();
    match names.len() {
        0 => None,
        1 => Some(format!("{one} {}", names[0])),
        _ => Some(format!("{many} {}", names.join(", "))),
    }
}

/// While it runs, a fact a line (the web's `whileItRuns`): its port, its run budget, the runs its
/// log keeps, its state, and its claims. A limit it asks for (`limits`) is its own; the rest are
/// the host's `defaults` (GET /permissions; null while unread).
pub(crate) fn while_it_runs(port: Option<u16>, touches: &Value, limits: &Value, defaults: &Value) -> Vec<(&'static str, String)> {
    let now = live(touches);
    let has = |kind: &str| now.iter().any(|t| t["kind"] == kind);
    let limit = |name: &str| -> Option<(u64, String)> {
        let host = defaults[name].as_u64();
        match limits[name].as_u64() {
            Some(own) => {
                let said = match host {
                    Some(h) if h != own => format!(" (its own; the host's is {})", if name.ends_with("_ms") { ms(h) } else { h.to_string() }),
                    _ => String::new(),
                };
                Some((own, said))
            }
            None => host.map(|h| (h, String::new())),
        }
    };
    let routes = has("requests") || has("websocket");
    let mut facts = vec![("port", if routes { port.map_or("the host's default".into(), |p| format!(":{p}")) } else { "none: nothing calls it over HTTP".into() })];
    let mut budgets = vec![];
    if let Some((n, said)) = limit("run_budget_ms").filter(|_| routes || !has("clock")) {
        budgets.push(format!("{} a run{said}", ms(n)));
    }
    if let Some((n, said)) = limit("clock_run_budget_ms").filter(|_| has("clock")) {
        budgets.push(format!("{} a clock run{said}", ms(n)));
    }
    facts.push(("run budget", if budgets.is_empty() { "the host's".into() } else { budgets.join("; ") }));
    let kept = match (limit("keep_runs"), limit("keep_run_days")) {
        (Some((n, said)), days) => format!("the newest {n}{said}{}", days.map(|(d, s)| format!(", none older than {d} days{s}")).unwrap_or_default()),
        (None, _) => "as the host keeps them".into(),
    };
    facts.push(("runs kept", kept));
    let state: Vec<&Value> = now.iter().copied().filter(|t| t["group"] == "state").collect();
    let held: Vec<String> = [named(&state, "key", "key", "keys"), named(&state, "collection", "collection", "collections")].into_iter().flatten().collect();
    facts.push(("its state", if held.is_empty() { "nothing: each run starts afresh".into() } else { held.join("; ") }));
    // A run holds what it read (a key, a row, a file) once it is about to call out, until it commits.
    let read = now.iter().any(|t| t["group"] == "state" || t["kind"] == "database" || t["kind"] == "folder");
    let calls = now.iter().any(|t| t["group"] == "outside");
    facts.push((
        "its claims",
        if read && calls { "what a run read before it calls out, held by that run until it commits: one run at a time calls out over the same key".into() } else { "none: no run reads what it keeps and then calls out".into() },
    ));
    facts
}

/// Whether the host grants `item` without asking (permissions.rs `is_auto`): `Kind(*)` covers
/// every item of its kind, any other pattern only itself. Never a tag.
fn auto_grants<'a>(auto: &'a Value, item: &str) -> Option<&'a str> {
    if item.starts_with("Tag(") {
        return None;
    }
    auto.as_array()?.iter().filter_map(Value::as_str).find(|p| match p.strip_suffix("(*)") {
        Some(kind) => item.starts_with(&format!("{kind}(")),
        None => *p == item,
    })
}

/// The lines of the host's policy a version relies on, each with whether it holds (the web's
/// `policyRows`): each host it calls is allowed (granted without asking, where `auto` says so),
/// plain HTTP and private addresses where it asks; each limit it raises against its ceiling; what
/// else the host grants without asking; and last, whether the caller may approve what it asks
/// (`flow.approve-deploy`) or deploy it (`flow.deploy`): `may`, as `can` answered. `policy` is GET
/// /permissions, None when the caller may not read it.
pub(crate) fn policy_rows(touches: &Value, items: &[String], limits: &Value, private_addresses: bool, policy: Option<&Value>, may: Result<(), String>, approving: bool) -> Vec<(bool, String)> {
    let mut rows = vec![];
    if let Some(policy) = policy {
        let (plain, private) = (policy["allow_plain_http"] == true, policy["allow_private_addresses"] == true);
        let mut hosts = std::collections::BTreeSet::new();
        for t in live(touches).into_iter().filter(|t| t["kind"] == "host") {
            let name = t["name"].as_str().unwrap_or("?");
            for u in t["uses"].as_array().into_iter().flatten().filter(|u| u["change"] != "gone") {
                let item = u["item"].as_str().unwrap_or_default();
                if item.starts_with("PlainHttp(") {
                    rows.push((plain, format!("plain HTTP to {name}: the host {}", if plain { "allows it" } else { "doesn't allow it" })));
                    continue;
                }
                hosts.insert(item.to_string());
                let auto = auto_grants(&policy["auto"], item).map(|a| format!(", and grants it without asking (auto {a})")).unwrap_or_default();
                rows.push((true, format!("it calls {name}: the host allows it{auto}")));
            }
        }
        if private_addresses {
            rows.push((private, format!("private addresses: the host {}", if private { "allows reaching them" } else { "doesn't allow reaching them" })));
        }
        for (name, n) in limits.as_object().into_iter().flatten().filter_map(|(k, v)| Some((k, v.as_u64()?))) {
            let Some(host) = policy["defaults"][name].as_u64().filter(|h| n > *h) else { continue };
            rows.push(match policy["ceilings"][name].as_u64() {
                None => (true, format!("{name} = {n}: above the host's {host}, with no ceiling")),
                Some(c) => (n <= c, format!("{name} = {n}: above the host's {host}, {} its ceiling of {c}", if n <= c { "within" } else { "past" })),
            });
        }
        for item in items.iter().filter(|i| !hosts.contains(*i)) {
            if let Some(a) = auto_grants(&policy["auto"], item) {
                rows.push((true, format!("{item}: the host grants it without asking (auto {a})")));
            }
        }
    }
    let action = if approving { "flow.approve-deploy" } else { "flow.deploy" };
    rows.push(match may {
        Ok(()) if approving => (true, format!("you may approve what it asks for ({action})")),
        Ok(()) => (true, format!("you may deploy it ({action})")),
        Err(why) => (false, why),
    });
    rows
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Each line as the web's tests have it (reviewMay.test.ts): a host allowed and granted without
    /// asking, plain HTTP refused, a limit past its ceiling, what the caller may do.
    #[test]
    fn the_policy_lines_say_what_holds_and_what_doesnt() {
        let touches = json!([
            { "kind": "host", "name": "api.example.com", "change": "new", "uses": [{ "item": "Network(\"api.example.com\")", "change": "new" }] },
            { "kind": "host", "name": "old.example.com", "change": "same", "uses": [{ "item": "PlainHttp(\"old.example.com\")", "change": "same" }] },
        ]);
        let policy = json!({ "auto": ["Network(*)"], "allow_plain_http": false, "allow_private_addresses": true, "defaults": { "keep_runs": 100 }, "ceilings": { "keep_runs": 500 } });
        let rows = policy_rows(&touches, &["Network(\"api.example.com\")".into()], &json!({ "keep_runs": 900 }), true, Some(&policy), Err("bob may not approve it".into()), true);
        let said: Vec<(bool, &str)> = rows.iter().map(|(ok, t)| (*ok, t.as_str())).collect();
        assert_eq!(
            said,
            vec![
                (true, "it calls api.example.com: the host allows it, and grants it without asking (auto Network(*))"),
                (false, "plain HTTP to old.example.com: the host doesn't allow it"),
                (true, "private addresses: the host allows reaching them"),
                (false, "keep_runs = 900: above the host's 100, past its ceiling of 500"),
                (false, "bob may not approve it"),
            ]
        );
        let unread = policy_rows(&touches, &[], &json!({}), false, None, Ok(()), false);
        assert_eq!(unread, vec![(true, "you may deploy it (flow.deploy)".to_string())]);
    }

    #[test]
    fn while_it_runs_says_its_own_limits_beside_the_hosts() {
        let touches = json!([{ "kind": "requests", "name": "GET /x", "change": "new" }, { "kind": "key", "name": "count", "group": "state", "change": "new" }, { "kind": "host", "name": "h", "group": "outside", "change": "new" }]);
        let facts = while_it_runs(Some(8101), &touches, &json!({ "run_budget_ms": 2000 }), &json!({ "run_budget_ms": 5000, "keep_runs": 100, "keep_run_days": 30 }));
        let said: Vec<String> = facts.iter().map(|(k, v)| format!("{k}: {v}")).collect();
        assert_eq!(
            said,
            vec![
                "port: :8101",
                "run budget: 2 s a run (its own; the host's is 5 s)",
                "runs kept: the newest 100, none older than 30 days",
                "its state: key count",
                "its claims: what a run read before it calls out, held by that run until it commits: one run at a time calls out over the same key",
            ]
        );
    }
}
