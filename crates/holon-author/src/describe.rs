//! What a manifest item (a capability's consent name, as core's `consent_name` writes it, or a
//! requirement such as `Limit(max_requests=80)`) lets a flow do, in words: what the editor's
//! deploy dialog and manifests show, and what `holon check` prints.

/// `item` in words; an item it doesn't know is said as it is.
pub fn describe_item(item: &str) -> String {
    let arg = |prefix: &str| -> Option<String> {
        let inner = item.strip_prefix(prefix)?.strip_prefix('(')?.strip_suffix(')')?;
        Some(inner.strip_prefix('"').and_then(|i| i.strip_suffix('"')).unwrap_or(inner).to_string())
    };
    type Say = fn(&str) -> String;
    let named: [(&str, Say); 9] = [
        ("HttpRequest", |a| format!("call https://{a}")),
        ("StateRead", |a| format!("read its state \"{a}\"")),
        ("StateWrite", |a| format!("write its state \"{a}\"")),
        ("CollectionRead", |a| format!("read items of its collection \"{a}\"")),
        ("CollectionWrite", |a| format!("put and delete items of its collection \"{a}\"")),
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
        "FileRead" => "read files",
        "FileWrite" => "write files",
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
        assert_eq!(describe_item("NetworkIngress"), "receive requests");
        assert_eq!(describe_item("Something(new)"), "Something(new)");
    }
}
