//! What a manifest item (a capability's consent name, as core's `consent_name` writes it, or a
//! requirement such as `Limit(max_requests=80)`) lets a flow do, in words: what the editor's
//! deploy dialog and manifests show, and what `holon check` prints.

use std::collections::{BTreeMap, BTreeSet};

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
    // How long its log keeps runs (docs/L4s/history-L4.md §7): said the same whether it is more
    // than the host's default or less.
    if let Some(n) = arg("Limit").and_then(|a| a.strip_prefix("keep_runs=").map(str::to_string)) {
        return format!("keep its newest {n} runs in its log");
    }
    if let Some(n) = arg("Limit").and_then(|a| a.strip_prefix("keep_run_days=").map(str::to_string)) {
        return format!("keep its runs in its log for up to {n} days");
    }
    type Say = fn(&str) -> String;
    let named: [(&str, Say); 34] = [
        ("HttpRequest", |a| format!("call https://{a}")),
        ("StateRead", |a| format!("read its state \"{a}\"")),
        ("StateWrite", |a| format!("write its state \"{a}\"")),
        ("CollectionRead", |a| format!("read items of its collection \"{a}\"")),
        ("CollectionWrite", |a| format!("put and delete items of its collection \"{a}\"")),
        ("FileRead", |a| format!("read files in the folder \"{a}\"")),
        ("FileArrival", |a| format!("run on each file new, changed or removed in the folder \"{a}\" (as its on file lines say), and pass it on")),
        ("Action", |a| format!("can be run by hand: {a}")),
        ("AgentRuns", |a| format!("let agents run its action {a}")),
        ("Unrecorded", |a| format!("keep runs of {a} out of its log, failed or not (they are counted)")),
        ("RunsAction", |a| match a.split_once('/') {
            Some((flow, action)) => format!("run the action {action} of the flow {flow}"),
            None => format!("run an action of the flow {a}"),
        }),
        ("FileWrite", |a| format!("write, copy into and delete files in the folder \"{a}\"")),
        ("FileServe", |a| format!("answer its callers with files from the folder \"{a}\"")),
        ("DbRead", |a| table_of(a, "read")),
        ("DbWrite", |a| table_of(a, "insert, update and delete rows of")),
        ("ChannelSend", |a| format!("send packs on its channel \"{a}\" to whoever subscribes")),
        ("ChannelRead", |a| format!("let callers subscribe to its channel \"{a}\"")),
        ("Log", |a| format!("write {a} lines to its log, which is kept apart from its runs")),
        ("MailSend", |a| match a.split_once('/') {
            Some((mail, "*")) => format!("send mail through \"{mail}\" to an address it computes, within its binding's"),
            Some((mail, to)) => format!("send mail through \"{mail}\" to {to}"),
            None => format!("send mail through \"{a}\""),
        }),
        ("MailArrival", |a| format!("run on each mail that comes to \"{a}\", read it, and move it once its run is over")),
        ("FeedEvent", |a| format!("run on each event from the feed \"{a}\"")),
        ("QueueMessage", |a| format!("run on each message from the queue \"{a}\", and delete it once its run is over")),
        ("QueueSend", |a| format!("send messages to the queue \"{a}\"")),
        ("WebSocket", |a| format!("hold WebSocket connections on {a}, and run on each message")),
        ("PlainHttp", |a| format!("call {a} over plain HTTP (not encrypted)")),
        ("SecretRead", |a| format!("read the secret \"{a}\"")),
        ("SecretAnswer", |a| format!("answer its callers with the secret \"{a}\", or with what is computed from it")),
        ("Await", |a| format!("wait for an event \"{a}\" and continue later")),
        ("Approval", |a| match a.split_once('/').and_then(|(name, steps)| Some((name, holon_core::approval::Steps::parse(steps)?))) {
            Some((name, steps)) => format!("wait for approval \"{name}\" by {}, and continue later", steps.said()),
            None => format!("wait for an approval \"{a}\""),
        }),
        ("Limit", |a| format!("raise {}", a.replacen('=', " to ", 1))),
        ("Browser", |a| if a == "*" { "answer web pages from any site".into() } else { format!("answer web pages from {a}") }),
        ("Tag", |a| format!("join the tag \"{a}\": whoever is bound on it acts on this flow")),
        ("Untaint", |a| format!("record the values of \"{a}\" again, which it keeps out of its records now")),
        ("KeepAgain", |a| format!("keep the values of \"{a}\" again, which it never keeps now")),
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
        "AgentReadsLogs" => "let agents read its log lines",
        "SecretAccess" => "read secrets",
        "Spawn" => "start processes",
        other => other,
    }
    .to_string()
}

/// Where a secret is sent, as core's `sent_secrets` names it, in words: a host as its address
/// (`https://api.example.com`), a database read's database as `the database "shop"`.
pub fn describe_destination(destination: &str) -> String {
    if destination == holon_core::CALLERS {
        return "its callers".to_string();
    }
    match destination.strip_prefix("db:") {
        Some(database) => format!("the database \"{database}\""),
        None => format!("https://{destination}"),
    }
}

/// Where a secret goes, in words: each part is there when it goes somewhere.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SentTo {
    /// Where the secret itself is sent: "https://api.example.com and the database "shop"".
    pub own: Option<String>,
    /// Where what is answered to it goes (its `answer:` destinations): "its callers and
    /// https://api.example.com".
    pub answers: Option<String>,
    /// Where a hash or a signature made with it goes (its `made:` destinations).
    pub made: Option<String>,
}

impl SentTo {
    /// What the approval says after "read the secret "k"": ", and send it to …; what is answered
    /// to it goes to …; a hash or signature made with it goes to …", each part when there is one;
    /// nothing for a secret that goes nowhere.
    pub fn said_after_read(&self) -> String {
        let mut said = self.own.as_ref().map(|own| format!(", and send it to {own}")).unwrap_or_default();
        for clause in self.clauses() {
            said.push_str(&format!("; {clause}"));
        }
        said
    }

    /// Where what is answered to it and what is made with it go, a clause each: "what is
    /// answered to it goes to …", "a hash or signature made with it goes to …".
    pub fn clauses(&self) -> Vec<String> {
        let answers = self.answers.as_ref().map(|to| format!("what is answered to it goes to {to}"));
        let made = self.made.as_ref().map(|to| format!("a hash or signature made with it goes to {to}"));
        answers.into_iter().chain(made).collect()
    }
}

/// Where a secret goes, as core's `sent_secrets` lists it, in words ([`describe_destination`]):
/// where it is sent itself, where what is answered to it goes (its `answer:` destinations) and
/// where a hash or a signature made with it goes (its `made:` ones), each when there is one.
pub fn describe_sent_to(to: &BTreeSet<String>) -> SentTo {
    let listed = |to: Vec<&str>| -> Option<String> {
        // Its callers, then the hosts, then the databases.
        let mut to = to;
        to.sort_by_key(|d| if *d == holon_core::CALLERS { 0 } else if d.starts_with("db:") { 2 } else { 1 });
        let to: Vec<String> = to.into_iter().map(describe_destination).collect();
        match to.split_last() {
            None => None,
            Some((last, rest)) if !rest.is_empty() => Some(format!("{} and {last}", rest.join(", "))),
            Some((last, _)) => Some(last.clone()),
        }
    };
    let marked = |mark: &str| listed(to.iter().filter_map(|d| d.strip_prefix(mark)).collect());
    SentTo { own: listed(to.iter().map(String::as_str).filter(|d| holon_core::is_own_destination(d)).collect()), answers: marked(holon_core::ANSWER), made: marked(holon_core::MADE) }
}

/// `item` in words (as [`describe_item`]), and beside a secret it reads, where the flow sends
/// it, where what is answered to it goes and where a hash or a signature made with it goes
/// (core's `sent_secrets`, [`describe_sent_to`]): "read the secret "k", and send it to
/// https://auth.example.com; what is answered to it goes to https://api.example.com". It isn't
/// an item of its own; the approval says it here so a person sees where each secret goes.
pub fn describe_item_sending(item: &str, sent: &BTreeMap<String, BTreeSet<String>>) -> String {
    let said = describe_item(item);
    let name = item.strip_prefix("SecretRead(\"").and_then(|i| i.strip_suffix("\")"));
    match name.and_then(|n| sent.get(n)) {
        Some(to) => format!("{said}{}", describe_sent_to(to).said_after_read()),
        None => said,
    }
}

/// What a sink does with the secret `secret` it is fed, as the canvas marks it: "sends the secret
/// "k" to https://api.example.com"; a write or a call the host refuses says so.
pub fn describe_secret_sink(secret: &str, sink: &holon_core::Capability) -> String {
    use holon_core::Capability::*;
    let what = describe_item(&holon_core::consent_name(sink));
    match sink {
        HttpRequest(host) => format!("sends the secret \"{secret}\" to https://{host}"),
        DbRead(table) => match table.split_once('/') {
            Some((database, table)) => format!("sends the secret \"{secret}\" to the database \"{database}\", reading its table \"{table}\""),
            None => format!("{what}, with the secret \"{secret}\""),
        },
        StateWrite(_) | CollectionWrite(_) | FileWrite(_) | DbWrite(_) | ChannelSend(_) => {
            format!("would keep the secret \"{secret}\" ({what}): the host refuses it")
        }
        MailSend(_) => format!("would mail the secret \"{secret}\" ({what}): the host refuses it"),
        RunsAction(_) => format!("would hand the secret \"{secret}\" to another flow ({what}): the host refuses it"),
        _ => format!("{what}, with the secret \"{secret}\""),
    }
}

/// What a route says of answering its caller with what is answered to the secret `secret`, as
/// the canvas marks it. No approval of its own: the secret's read says where it goes.
pub fn describe_answer_answered(secret: &str) -> String {
    format!("answers its caller with what is answered to the secret \"{secret}\"")
}

/// What a route says of answering its caller with a hash or a signature made with the secret
/// `secret`, as the canvas marks it.
pub fn describe_made_answered(secret: &str) -> String {
    format!("answers its caller with a hash or signature made with the secret \"{secret}\"")
}

/// What a sink does with a hash or a signature made with the secret `secret` (core's
/// `SecretFed::made_of`), as the canvas marks it: "sends a hash or signature made with the secret
/// "k" to https://api.example.com". Nothing is refused: it isn't the secret.
pub fn describe_made_sink(secret: &str, sink: &holon_core::Capability) -> String {
    use holon_core::Capability::*;
    let what = describe_item(&holon_core::consent_name(sink));
    let made = format!("a hash or signature made with the secret \"{secret}\"");
    match sink {
        HttpRequest(host) => format!("sends {made} to https://{host}"),
        DbRead(table) => match table.split_once('/') {
            Some((database, table)) => format!("sends {made} to the database \"{database}\", reading its table \"{table}\""),
            None => format!("{what}, with {made}"),
        },
        StateWrite(_) | CollectionWrite(_) | FileWrite(_) | DbWrite(_) | ChannelSend(_) => format!("keeps {made} ({what})"),
        MailSend(_) => format!("mails {made} ({what})"),
        RunsAction(_) => format!("hands {made} to another flow ({what})"),
        _ => format!("{what}, with {made}"),
    }
}

/// What a sink does with what is answered to the secret `secret` (core's
/// `SecretFed::answers_of`: the token a token exchange answers, an API's echo), as the canvas
/// marks it: "sends what is answered to the secret "k" to https://api.example.com".
pub fn describe_answer_sink(secret: &str, sink: &holon_core::Capability) -> String {
    use holon_core::Capability::*;
    let what = describe_item(&holon_core::consent_name(sink));
    let answered = format!("what is answered to the secret \"{secret}\"");
    match sink {
        HttpRequest(host) => format!("sends {answered} to https://{host}"),
        DbRead(table) => match table.split_once('/') {
            Some((database, table)) => format!("sends {answered} to the database \"{database}\", reading its table \"{table}\""),
            None => format!("{what}, with {answered}"),
        },
        StateWrite(_) | CollectionWrite(_) | FileWrite(_) | DbWrite(_) | ChannelSend(_) => format!("keeps {answered} ({what}); a token in it is refused"),
        MailSend(_) => format!("mails {answered} ({what}); a token in it is refused"),
        _ => format!("{what}, with {answered}"),
    }
}

/// A route's answer that carries the secret `secret`, as the canvas marks the node that makes it.
pub fn describe_secret_answer(secret: &str) -> String {
    format!("answers its caller with the secret \"{secret}\" (approved apart, at deploy)")
}

/// A database capability's argument (`shop/orders`) said as what it lets a flow do: `read table
/// "orders" of database "shop"`.
fn table_of(arg: &str, verb: &str) -> String {
    match arg.split_once('/') {
        Some((database, table)) => format!("{verb} the table \"{table}\" of the database \"{database}\""),
        None => format!("{verb} the database \"{arg}\""),
    }
}

#[cfg(test)]
mod tests {
    use super::{describe_item, describe_item_sending};
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn says_where_a_secret_is_sent_beside_it() {
        let sent = BTreeMap::from([("k".to_string(), BTreeSet::from(["a.example.com".to_string(), "b.example.com".to_string()]))]);
        assert_eq!(describe_item_sending(r#"SecretRead("k")"#, &sent), r#"read the secret "k", and send it to https://a.example.com and https://b.example.com"#);
        assert_eq!(describe_item_sending(r#"SecretRead("j")"#, &sent), r#"read the secret "j""#, "a secret sent nowhere");
        let exchanged = BTreeMap::from([("k".to_string(), BTreeSet::from(["auth.example.com".to_string(), "answer:api.example.com".to_string(), "answer:db:shop".to_string()]))]);
        assert_eq!(
            describe_item_sending(r#"SecretRead("k")"#, &exchanged),
            r#"read the secret "k", and send it to https://auth.example.com; what is answered to it goes to https://api.example.com and the database "shop""#,
            "a token exchange"
        );
        let api = holon_core::Capability::HttpRequest("api.example.com".into());
        assert_eq!(super::describe_answer_sink("k", &api), r#"sends what is answered to the secret "k" to https://api.example.com"#);
        let queried = BTreeMap::from([("k".to_string(), BTreeSet::from(["db:audit".to_string(), "hooks.example.com".to_string()]))]);
        assert_eq!(describe_item_sending(r#"SecretRead("k")"#, &queried), r#"read the secret "k", and send it to https://hooks.example.com and the database "audit""#, "a database read's parameters");
        let read = holon_core::Capability::DbRead("audit/log".into());
        assert_eq!(super::describe_secret_sink("k", &read), r#"sends the secret "k" to the database "audit", reading its table "log""#);
    }

    #[test]
    fn says_each_item_in_words() {
        assert_eq!(describe_item(r#"HttpRequest("hooks.slack.com")"#), "call https://hooks.slack.com");
        assert_eq!(describe_item(r#"SecretRead("api_token")"#), r#"read the secret "api_token""#);
        assert_eq!(describe_item(r#"SecretAnswer("api_token")"#), r#"answer its callers with the secret "api_token", or with what is computed from it"#);
        assert_eq!(describe_item("Limit(max_requests=80)"), "raise max_requests to 80");
        assert_eq!(describe_item("Limit(keep_runs=2000)"), "keep its newest 2000 runs in its log");
        assert_eq!(describe_item("Limit(keep_run_days=30)"), "keep its runs in its log for up to 30 days");
        assert_eq!(describe_item(r#"DbRead("shop/orders")"#), r#"read the table "orders" of the database "shop""#);
        assert_eq!(describe_item(r#"DbWrite("shop/public.orders")"#), r#"insert, update and delete rows of the table "public.orders" of the database "shop""#);
        assert_eq!(describe_item(r#"MailSend("ops/approvals@acme.com")"#), r#"send mail through "ops" to approvals@acme.com"#);
        assert_eq!(describe_item(r#"MailSend("ops/*")"#), r#"send mail through "ops" to an address it computes, within its binding's"#);
        assert_eq!(describe_item(r#"MailArrival("remit")"#), r#"run on each mail that comes to "remit", read it, and move it once its run is over"#);
        assert_eq!(describe_item(r#"FeedEvent("prices")"#), r#"run on each event from the feed "prices""#);
        assert_eq!(describe_item(r#"QueueMessage("payouts")"#), r#"run on each message from the queue "payouts", and delete it once its run is over"#);
        assert_eq!(describe_item(r#"WebSocket("/room/:name")"#), "hold WebSocket connections on /room/:name, and run on each message");
        assert_eq!(describe_item(r#"QueueSend("retries")"#), r#"send messages to the queue "retries""#);
        assert_eq!(describe_item(r#"Approval("payout/person*2")"#), r#"wait for approval "payout" by 2 people, and continue later"#);
        assert_eq!(describe_item(r#"Approval("payout/person")"#), r#"wait for approval "payout" by a person, and continue later"#);
        assert_eq!(
            describe_item(r#"Approval("payout/person:a+person:b>person:c")"#),
            r#"wait for approval "payout" by a person holding a and a person holding b, in any order, then a person holding c, and continue later"#
        );
        // A module's budgets read as what the code may consume, not as a limit raised.
        assert_eq!(describe_item("Limit(module_fuel=200000000)"), "run code it carries (a js node) for up to 200000000 instructions per call, reaching nothing outside");
        assert_eq!(describe_item("Limit(module_memory=16777216)"), "give that code up to 16 MiB of memory per call");
        assert_eq!(describe_item("NetworkIngress"), "receive requests");
        assert_eq!(describe_item(r#"Action("reset")"#), "can be run by hand: reset");
        assert_eq!(describe_item(r#"AgentRuns("reset")"#), "let agents run its action reset");
        assert_eq!(describe_item("AgentReadsLogs"), "let agents read its log lines");
        assert_eq!(describe_item(r#"Unrecorded("GET /health")"#), "keep runs of GET /health out of its log, failed or not (they are counted)");
        assert_eq!(describe_item("Something(new)"), "Something(new)");
    }
}

