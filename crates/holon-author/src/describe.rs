//! What a manifest item (a capability's consent name, as core's `consent_name` writes it, or a
//! requirement such as `Limit(max_requests=80)`) lets a flow do, in words: what the editor's
//! deploy dialog and manifests show, and what `holon check` prints.

/// `item` in words; an item it doesn't know is said as it is.
pub fn describe_item(item: &str) -> String {
    let arg = |prefix: &str| -> Option<String> {
        let inner = item.strip_prefix(prefix)?.strip_prefix('(')?.strip_suffix(')')?;
        Some(inner.strip_prefix('"').and_then(|i| i.strip_suffix('"')).unwrap_or(inner).to_string())
    };
    // A module's budgets (docs/L4s/wasm-module-L4.md §4): what the code a flow carries may consume.
    if let Some(n) = arg("Limit").and_then(|a| a.strip_prefix(&format!("{}=", holon_core::FUEL_LIMIT)).map(str::to_string)) {
        return format!("run code it carries (a js node) for up to {n} instructions per call, reaching nothing outside");
    }
    if let Some(n) = arg("Limit").and_then(|a| a.strip_prefix(&format!("{}=", holon_core::MEMORY_LIMIT)).and_then(|v| v.parse::<u64>().ok())) {
        return format!("give that code up to {} MiB of memory per call", n / (1024 * 1024));
    }
    type Say = fn(&str) -> String;
    let named: [(&str, Say); 14] = [
        ("HttpRequest", |a| format!("call https://{a}")),
        ("StateRead", |a| format!("read its state \"{a}\"")),
        ("StateWrite", |a| format!("write its state \"{a}\"")),
        ("CollectionRead", |a| format!("read items of its collection \"{a}\"")),
        ("CollectionWrite", |a| format!("put and delete items of its collection \"{a}\"")),
        ("FileRead", |a| format!("read files in the folder \"{a}\"")),
        ("FileArrival", |a| format!("run on each file new, changed or removed in the folder \"{a}\" (as its on file lines say), and pass it on")),
        ("FileWrite", |a| format!("write, copy into and delete files in the folder \"{a}\"")),
        ("ChannelSend", |a| format!("send packs on its channel \"{a}\" to whoever subscribes")),
        ("ChannelRead", |a| format!("let callers subscribe to its channel \"{a}\"")),
        ("PlainHttp", |a| format!("call {a} over plain HTTP (not encrypted)")),
        ("SecretRead", |a| format!("read the secret \"{a}\"")),
        ("Await", |a| format!("wait for an event \"{a}\" and continue later")),
        ("Limit", |a| format!("raise {}", a.replacen('=', " to ", 1))),
    ];
    for (prefix, say) in named {
        if let Some(a) = arg(prefix) {
            return say(&a);
        }
    }
    match item {
        "NetworkIngress" => "receive requests",
        "NetworkEgress" => "send outbound messages",
        "Clock" => "read the clock",
        "Random" => "use randomness",
        "PrivateAddresses" => "reach private addresses (this machine and its network)",
        "SecretAccess" => "read secrets",
        "Spawn" => "start processes",
        other => other,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::describe_item;

    #[test]
    fn says_each_item_in_words() {
        assert_eq!(describe_item(r#"HttpRequest("hooks.slack.com")"#), "call https://hooks.slack.com");
        assert_eq!(describe_item(r#"SecretRead("api_token")"#), r#"read the secret "api_token""#);
        assert_eq!(describe_item("Limit(max_requests=80)"), "raise max_requests to 80");
        // A module's budgets read as what the code may consume, not as a limit raised.
        assert_eq!(describe_item("Limit(module_fuel=200000000)"), "run code it carries (a js node) for up to 200000000 instructions per call, reaching nothing outside");
        assert_eq!(describe_item("Limit(module_memory=16777216)"), "give that code up to 16 MiB of memory per call");
        assert_eq!(describe_item("NetworkIngress"), "receive requests");
        assert_eq!(describe_item("Something(new)"), "Something(new)");
    }
}
