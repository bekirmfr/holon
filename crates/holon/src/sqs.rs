//! A queue's SQS (host-capabilities L4 §5.9.9): the client a queue's binding (queues.rs) and its
//! listener (dequeue.rs) talk to it through, in SQS's JSON protocol (`application/x-amz-json-1.0`,
//! `X-Amz-Target: AmazonSQS.<Action>`), which AWS answers and ElasticMQ and LocalStack copy, each
//! request signed with AWS's Signature Version 4 (sigv4.rs).
//!
//! - **The URL** is the queue's own (`QueueUrl`), its login the access key id and secret,
//!   percent-encoded: `https://AKIA…:<secret>@sqs.eu-west-1.amazonaws.com/123456789012/payouts`.
//!   The region is the host's, or `?region=` for a service under another name; `?token=` is a
//!   temporary key's session token. Plain `http://` only to this machine.
//! - **Five actions:** `GetQueueAttributes` (whether it is FIFO, its largest message),
//!   `SendMessage`, `ReceiveMessage` (a long poll), `DeleteMessage` and `ChangeMessageVisibility`.
//! - **What SQS answers** is temporary when it is worth trying again (no connection, a 5xx, a
//!   throttle), and permanent otherwise (a wrong key, a queue that isn't there), its `__type`
//!   and message said: or its `<Code>` and `<Message>`, as a refusal in the query protocol's XML
//!   says them (moto refuses a key so).

use std::collections::BTreeMap;
use std::time::Duration;

use zeroize::Zeroizing;

use crate::sigv4::{self, Keys, Scope};
use crate::smtp::{unpercent, Failure};

/// How long a request may take: a long poll's 20 s, and some.
const TIMEOUT: Duration = Duration::from_secs(45);

/// How much of a refusal's message is said.
const MAX_SAID: usize = 200;

const USAGE: &str = "a queue's URL is https://<key id>:<secret>@sqs.<region>.amazonaws.com/<account>/<queue>, the secret percent-encoded";

/// A queue, as its binding's URL names it: where it is and the keys its requests are signed with.
pub(crate) struct Queue {
    /// The queue's URL without its login or query: what `QueueUrl` says, and what is shown.
    url: String,
    /// Where requests go: the scheme and the host, `https://sqs.eu-west-1.amazonaws.com`.
    endpoint: String,
    /// The host as the `Host` header says it, its port only when it isn't the scheme's.
    host: String,
    region: String,
    keys: Keys,
}

/// What a queue says of itself when it is bound.
#[derive(Debug, PartialEq)]
pub(crate) struct Attributes {
    pub(crate) fifo: bool,
    /// Its largest message, in bytes (`MaximumMessageSize`).
    pub(crate) largest: u64,
}

/// A message received: what `m` is made of, and the receipt that deletes it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Received {
    pub(crate) id: String,
    pub(crate) receipt: String,
    pub(crate) body: String,
    /// Its message attributes of type `String` or `Number`.
    pub(crate) attributes: BTreeMap<String, String>,
    /// When it was sent, in milliseconds since 1970.
    pub(crate) sent_ms: u64,
    /// How many times the queue has handed it out.
    pub(crate) count: u64,
    /// A FIFO queue's message group.
    pub(crate) group: Option<String>,
}

/// Whether `host` (`127.0.0.1:8080`, `[::1]`, `localhost`) is this machine.
fn is_loopback(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(h, _)| h),
    };
    name == "localhost" || name.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The region an AWS host names: `sqs.eu-west-1.amazonaws.com`, `sqs-fips.us-east-1.amazonaws.com`,
/// or the older `eu-west-1.queue.amazonaws.com`.
fn region_of(host: &str) -> Option<String> {
    let name = host.rsplit_once(':').map_or(host, |(h, _)| h);
    let rest = name.strip_suffix(".amazonaws.com").or_else(|| name.strip_suffix(".amazonaws.com.cn"))?;
    let labels: Vec<&str> = rest.split('.').collect();
    match labels.as_slice() {
        [service, region] if service.starts_with("sqs") => Some(region.to_string()),
        [region, "queue"] => Some(region.to_string()),
        _ => None,
    }
}

impl Queue {
    /// The queue `url` names, or why it names none.
    pub(crate) fn parse(url: &str) -> Result<Queue, String> {
        let (scheme, rest) = url.split_once("://").ok_or(USAGE)?;
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "https" && scheme != "http" {
            return Err(format!("a queue's URL is https://… (or http:// to this machine), not {scheme}://"));
        }
        let rest = rest.split('#').next().unwrap_or("");
        let (before, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (authority, path) = match before.split_once('/') {
            Some((a, p)) => (a, format!("/{p}")),
            None => (before, "/".to_string()),
        };
        let (user_info, host) = authority.rsplit_once('@').ok_or("a queue's URL logs in with its access key: https://<key id>:<secret>@…")?;
        let (id, secret) = user_info.split_once(':').ok_or("a queue's URL logs in with its access key: https://<key id>:<secret>@…")?;
        let (id, secret) = (unpercent(id)?, unpercent(secret)?);
        if id.is_empty() || secret.is_empty() {
            return Err("a queue's URL logs in with an access key id and its secret, and gives none".into());
        }
        let host = host.to_ascii_lowercase();
        if host.is_empty() {
            return Err("a queue's URL names no host".into());
        }
        if scheme == "http" && !is_loopback(&host) {
            return Err("a queue is reached over https://: plain http:// only to this machine".into());
        }
        if path.trim_end_matches('/').is_empty() {
            return Err("a queue's URL is the queue's own: https://sqs.<region>.amazonaws.com/<account>/<queue>".into());
        }
        let (mut region, mut token) = (None, None);
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = unpercent(value).map_err(|_| format!("the queue's URL's {key} has a % that isn't %XX"))?;
            match key {
                "region" => region = Some(value.to_string()),
                "token" => token = Some(value),
                _ => return Err(format!("a queue's URL's query says {key:?}: it takes region and token")),
            }
        }
        let region = region.or_else(|| region_of(&host)).ok_or("the queue's host names no region (sqs.<region>.amazonaws.com): give it with ?region=")?;
        let host = match scheme.as_str() {
            "https" => host.strip_suffix(":443").unwrap_or(&host).to_string(),
            _ => host.strip_suffix(":80").unwrap_or(&host).to_string(),
        };
        Ok(Queue {
            url: format!("{scheme}://{host}{path}"),
            endpoint: format!("{scheme}://{host}"),
            host,
            region,
            keys: Keys { id: id.to_string(), secret: Zeroizing::new(secret.to_string()), token },
        })
    }

    /// Where it is, without its login or query.
    pub(crate) fn address(&self) -> String {
        self.url.clone()
    }

    /// One action: its request signed and sent, its answer read.
    fn call(&self, action: &str, request: serde_json::Value) -> Result<serde_json::Value, Failure> {
        let body = serde_json::to_vec(&request).expect("a request is JSON");
        let date = sigv4::amz_date_now();
        let target = format!("AmazonSQS.{action}");
        let mut headers = vec![("Content-Type", "application/x-amz-json-1.0"), ("Host", self.host.as_str()), ("X-Amz-Date", date.as_str()), ("X-Amz-Target", target.as_str())];
        if let Some(token) = &self.keys.token {
            headers.push(("X-Amz-Security-Token", token.as_str()));
        }
        let auth = sigv4::authorization(&self.keys, &Scope { region: &self.region, service: "sqs" }, &date, "POST", "/", "", &headers, &body);
        // A queue's service that sends its signed requests on elsewhere isn't followed.
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout(TIMEOUT).redirects(0).build();
        let mut req = agent.post(&format!("{}/", self.endpoint));
        for (name, value) in &headers {
            req = req.set(name, value);
        }
        let at = &self.url;
        match req.set("Authorization", &auth).send_bytes(&body) {
            Ok(answer) => {
                let text = answer.into_string().map_err(|e| Failure::Temporary(format!("{at}'s answer broke off: {e}")))?;
                if text.trim().is_empty() {
                    return Ok(serde_json::Value::Object(Default::default()));
                }
                serde_json::from_str(&text).map_err(|_| Failure::Permanent(format!("{at} answered {action} with something that isn't JSON")))
            }
            Err(ureq::Error::Status(code, answer)) => Err(refused(at, action, code, answer)),
            Err(ureq::Error::Transport(t)) => Err(Failure::Temporary(format!("{at} can't be reached: {}", t.message().map_or_else(|| t.kind().to_string(), str::to_string)))),
        }
    }

    /// Whether it is FIFO, and its largest message.
    pub(crate) fn attributes(&self) -> Result<Attributes, Failure> {
        let answer = self.call("GetQueueAttributes", serde_json::json!({ "QueueUrl": self.url, "AttributeNames": ["All"] }))?;
        let a = &answer["Attributes"];
        let fifo = a["FifoQueue"].as_str() == Some("true") || self.url.ends_with(".fifo");
        let largest = a["MaximumMessageSize"].as_str().and_then(|m| m.parse().ok()).unwrap_or(262_144);
        Ok(Attributes { fifo, largest })
    }

    /// Send `body`: in `group` with the deduplication id `dedup` to a FIFO queue. Its message id.
    pub(crate) fn send(&self, body: &str, group: Option<&str>, dedup: Option<&str>) -> Result<String, Failure> {
        let mut request = serde_json::json!({ "QueueUrl": self.url, "MessageBody": body });
        if let Some(g) = group {
            request["MessageGroupId"] = g.into();
        }
        if let Some(d) = dedup {
            request["MessageDeduplicationId"] = d.into();
        }
        let answer = self.call("SendMessage", request)?;
        Ok(answer["MessageId"].as_str().unwrap_or_default().to_string())
    }

    /// Up to `max` messages, waiting up to `wait` seconds for one, each kept out of sight for
    /// `visibility` seconds.
    pub(crate) fn receive(&self, max: u32, wait: u32, visibility: u32) -> Result<Vec<Received>, Failure> {
        let answer = self.call(
            "ReceiveMessage",
            serde_json::json!({
                "QueueUrl": self.url, "MaxNumberOfMessages": max, "WaitTimeSeconds": wait, "VisibilityTimeout": visibility,
                "AttributeNames": ["All"], "MessageSystemAttributeNames": ["All"], "MessageAttributeNames": ["All"],
            }),
        )?;
        let messages = answer["Messages"].as_array().cloned().unwrap_or_default();
        Ok(messages.iter().filter_map(received).collect())
    }

    /// Delete the message `receipt` names.
    pub(crate) fn delete(&self, receipt: &str) -> Result<(), Failure> {
        self.call("DeleteMessage", serde_json::json!({ "QueueUrl": self.url, "ReceiptHandle": receipt })).map(|_| ())
    }

    /// Keep the message `receipt` names out of sight for `visibility` seconds from now.
    pub(crate) fn extend(&self, receipt: &str, visibility: u32) -> Result<(), Failure> {
        self.call("ChangeMessageVisibility", serde_json::json!({ "QueueUrl": self.url, "ReceiptHandle": receipt, "VisibilityTimeout": visibility })).map(|_| ())
    }
}

/// A message as `ReceiveMessage` gives it: none without an id or a receipt.
fn received(m: &serde_json::Value) -> Option<Received> {
    let number = |v: &serde_json::Value| v.as_str().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let attributes = m["MessageAttributes"]
        .as_object()
        .map(|a| {
            a.iter()
                .filter(|(_, v)| v["DataType"].as_str().is_some_and(|t| t.starts_with("String") || t.starts_with("Number")))
                .filter_map(|(k, v)| Some((k.clone(), v["StringValue"].as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Some(Received {
        id: m["MessageId"].as_str()?.to_string(),
        receipt: m["ReceiptHandle"].as_str()?.to_string(),
        body: m["Body"].as_str().unwrap_or_default().to_string(),
        attributes,
        sent_ms: number(&m["Attributes"]["SentTimestamp"]),
        count: number(&m["Attributes"]["ApproximateReceiveCount"]),
        group: m["Attributes"]["MessageGroupId"].as_str().map(str::to_string),
    })
}

fn refused(at: &str, action: &str, code: u16, answer: ureq::Response) -> Failure {
    said(at, action, code, &answer.into_string().unwrap_or_default())
}

/// What a refusal `body` says: its `__type` (the part after `#`) and its message, or in XML its
/// `<Code>` and `<Message>`, cut short. A 5xx, a 429 or a throttle is worth trying again.
fn said(at: &str, action: &str, code: u16, body: &str) -> Failure {
    let json: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let xml = |tag: &str| body.split_once(&format!("<{tag}>")).and_then(|(_, rest)| rest.split_once(&format!("</{tag}>"))).map(|(inner, _)| inner.trim());
    let kind = json["__type"].as_str().map(|t| t.rsplit('#').next().unwrap_or(t)).or_else(|| xml("Code")).unwrap_or_default().to_string();
    let message = json["message"].as_str().or(json["Message"].as_str()).or_else(|| xml("Message")).unwrap_or_default();
    let message: String = message.lines().next().unwrap_or_default().chars().take(MAX_SAID).collect();
    let said = match (kind.is_empty(), message.is_empty()) {
        (true, true) => format!("{at} answered {code} to {action}"),
        (false, true) => format!("{at} answered {code} to {action}: {kind}"),
        (true, false) => format!("{at} answered {code} to {action}: {message}"),
        (false, false) => format!("{at} answered {code} to {action}: {kind}: {message}"),
    };
    if code >= 500 || code == 429 || kind.contains("Throttl") || kind.contains("RequestThrottled") {
        Failure::Temporary(said)
    } else {
        Failure::Permanent(said)
    }
}

/// An SQS on loopback for the tests: queues in memory, standard and FIFO (a name ending `.fifo`),
/// with visibility, receive counts, groups and deduplication, refusing a request whose signature
/// isn't the one its secret makes.
#[cfg(test)]
pub(crate) mod stand_in {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use rouille::{Request, Response};

    use crate::sigv4::{self, Keys, Scope};

    pub(crate) const KEY_ID: &str = "AKIASTANDIN";
    pub(crate) const SECRET: &str = "s3cret/With+Slash";

    #[derive(Clone, Debug)]
    pub(crate) struct Message {
        pub(crate) id: String,
        pub(crate) body: String,
        pub(crate) group: Option<String>,
        pub(crate) attributes: BTreeMap<String, String>,
        dedup: Option<String>,
        sent_ms: u64,
        count: u64,
        receipt: Option<String>,
        visible_at: Instant,
    }

    #[derive(Default)]
    struct Inner {
        /// Each queue's messages, in the order sent, by its name.
        queues: Mutex<BTreeMap<String, VecDeque<Message>>>,
        /// Each request's action, and its queue's name.
        asked: Mutex<Vec<String>>,
        /// Answers to give before any other: a status and a `__type`.
        failing: Mutex<VecDeque<(u16, String)>>,
        made: Mutex<u64>,
    }

    pub(crate) struct StandIn {
        host: String,
        inner: Arc<Inner>,
    }

    impl StandIn {
        pub(crate) fn start() -> StandIn {
            let inner = Arc::new(Inner::default());
            let shared = Arc::clone(&inner);
            let server = rouille::Server::new("127.0.0.1:0", move |request| answer(&shared, request)).unwrap();
            let host = server.server_addr().to_string();
            std::thread::spawn(move || server.run());
            StandIn { host, inner }
        }

        /// The URL of its queue `name`, logged in with its key, the region given.
        pub(crate) fn url(&self, name: &str) -> String {
            format!("http://{KEY_ID}:{}@{}/000000000000/{name}?region=stand-in", SECRET.replace('/', "%2F").replace('+', "%2B"), self.host)
        }

        /// Make its queue `name`.
        pub(crate) fn make(&self, name: &str) {
            self.inner.queues.lock().unwrap().entry(name.to_string()).or_default();
        }

        /// Put a message in `name`, as a sender outside would: its id.
        pub(crate) fn put(&self, name: &str, body: &str, group: Option<&str>) -> String {
            let mut queues = self.inner.queues.lock().unwrap();
            let id = self.inner.next_id();
            queues.entry(name.to_string()).or_default().push_back(Message::new(id.clone(), body, group, None));
            id
        }

        /// Put message `id` in `name` again, as a standard queue may hand one out twice.
        pub(crate) fn again(&self, name: &str, id: &str, body: &str) {
            self.inner.queues.lock().unwrap().entry(name.to_string()).or_default().push_back(Message::new(id.to_string(), body, None, None));
        }

        /// What `name` holds, in the order sent.
        pub(crate) fn messages(&self, name: &str) -> Vec<Message> {
            self.inner.queues.lock().unwrap().get(name).cloned().unwrap_or_default().into()
        }

        /// Each request so far: its action and its queue's name.
        pub(crate) fn asked(&self) -> Vec<String> {
            self.inner.asked.lock().unwrap().clone()
        }

        /// Answer the next requests with these refusals.
        pub(crate) fn fail(&self, answers: &[(u16, &str)]) {
            self.inner.failing.lock().unwrap().extend(answers.iter().map(|(c, t)| (*c, t.to_string())));
        }

        /// Make every message of `name` that waits out of sight visible now.
        pub(crate) fn expire(&self, name: &str) {
            for m in self.inner.queues.lock().unwrap().get_mut(name).into_iter().flatten() {
                m.visible_at = Instant::now();
            }
        }
    }

    impl Message {
        fn new(id: String, body: &str, group: Option<&str>, dedup: Option<&str>) -> Message {
            let sent_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
            Message { id, body: body.into(), group: group.map(Into::into), attributes: BTreeMap::new(), dedup: dedup.map(Into::into), sent_ms, count: 0, receipt: None, visible_at: Instant::now() }
        }
    }

    impl Inner {
        fn next_id(&self) -> String {
            let mut made = self.made.lock().unwrap();
            *made += 1;
            format!("m-{:04}", *made)
        }
    }

    fn refusal(code: u16, kind: &str, message: &str) -> Response {
        Response::from_data("application/x-amz-json-1.0", serde_json::json!({ "__type": format!("com.amazonaws.sqs#{kind}"), "message": message }).to_string()).with_status_code(code)
    }

    /// Whether `request`'s signature is the one its secret makes over what it sent.
    fn signed(request: &Request, body: &[u8]) -> bool {
        let Some(auth) = request.header("Authorization") else { return false };
        let Some(date) = request.header("X-Amz-Date") else { return false };
        let Some(signed) = auth.split("SignedHeaders=").nth(1).and_then(|s| s.split(',').next()) else { return false };
        let Some(scope) = auth.split("Credential=").nth(1).and_then(|s| s.split(',').next()) else { return false };
        let parts: Vec<&str> = scope.split('/').collect();
        if parts.len() != 5 || parts[0] != KEY_ID {
            return false;
        }
        let headers: Vec<(&str, &str)> = signed.split(';').filter_map(|n| request.headers().find(|(h, _)| h.eq_ignore_ascii_case(n)).map(|(_, v)| (n, v))).collect();
        let keys = Keys { id: KEY_ID.into(), secret: zeroize::Zeroizing::new(SECRET.into()), token: None };
        sigv4::authorization(&keys, &Scope { region: parts[2], service: parts[3] }, date, "POST", "/", "", &headers, body) == auth
    }

    fn answer(inner: &Inner, request: &Request) -> Response {
        let mut body = Vec::new();
        if let Some(mut b) = request.data() {
            let _ = std::io::Read::read_to_end(&mut b, &mut body);
        }
        let action = request.header("X-Amz-Target").and_then(|t| t.strip_prefix("AmazonSQS.")).unwrap_or_default().to_string();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let name = json["QueueUrl"].as_str().and_then(|u| u.rsplit('/').next()).unwrap_or_default().to_string();
        inner.asked.lock().unwrap().push(format!("{action} {name}"));
        if !signed(request, &body) {
            return refusal(403, "SignatureDoesNotMatch", "The request signature we calculated does not match the signature you provided.");
        }
        if let Some((code, kind)) = inner.failing.lock().unwrap().pop_front() {
            return refusal(code, &kind, "as the test asked");
        }
        let ok = |v: serde_json::Value| Response::from_data("application/x-amz-json-1.0", v.to_string());
        let fifo = name.ends_with(".fifo");
        // A long poll waits for a message, a second at most here.
        let until = Instant::now() + Duration::from_secs(json["WaitTimeSeconds"].as_u64().unwrap_or(0).min(1));
        loop {
            let now = Instant::now();
            let mut queues = inner.queues.lock().unwrap();
            let Some(queue) = queues.get_mut(&name) else {
                return refusal(400, "QueueDoesNotExist", "The specified queue does not exist.");
            };
            match action.as_str() {
                "GetQueueAttributes" => {
                    let mut a = serde_json::json!({ "VisibilityTimeout": "30", "MaximumMessageSize": "262144" });
                    if fifo {
                        a["FifoQueue"] = "true".into();
                    }
                    return ok(serde_json::json!({ "Attributes": a }));
                }
                "SendMessage" => {
                    let group = json["MessageGroupId"].as_str();
                    if fifo && group.is_none() {
                        return refusal(400, "MissingParameter", "The request must contain the parameter MessageGroupId.");
                    }
                    let dedup = json["MessageDeduplicationId"].as_str();
                    if let Some(same) = queue.iter().find(|m| fifo && dedup.is_some() && m.dedup.as_deref() == dedup) {
                        return ok(serde_json::json!({ "MessageId": same.id }));
                    }
                    let id = inner.next_id();
                    queue.push_back(Message::new(id.clone(), json["MessageBody"].as_str().unwrap_or_default(), group, dedup));
                    return ok(serde_json::json!({ "MessageId": id }));
                }
                "ReceiveMessage" => {
                    let max = json["MaxNumberOfMessages"].as_u64().unwrap_or(1) as usize;
                    let visibility = Duration::from_secs(json["VisibilityTimeout"].as_u64().unwrap_or(30));
                    // A FIFO queue hands out none of a group that has one out of sight.
                    let held: Vec<Option<String>> = queue.iter().filter(|m| fifo && m.visible_at > now).map(|m| m.group.clone()).collect();
                    let mut given = Vec::new();
                    for m in queue.iter_mut().filter(|m| m.visible_at <= now) {
                        if given.len() == max || held.contains(&m.group) {
                            continue;
                        }
                        m.count += 1;
                        m.visible_at = now + visibility;
                        let receipt = format!("{}-r{}", m.id, m.count);
                        m.receipt = Some(receipt.clone());
                        let mut attributes = serde_json::json!({ "SentTimestamp": m.sent_ms.to_string(), "ApproximateReceiveCount": m.count.to_string() });
                        if let Some(g) = &m.group {
                            attributes["MessageGroupId"] = g.clone().into();
                        }
                        let message_attributes: serde_json::Map<String, serde_json::Value> =
                            m.attributes.iter().map(|(k, v)| (k.clone(), serde_json::json!({ "DataType": "String", "StringValue": v }))).collect();
                        given.push(serde_json::json!({ "MessageId": m.id, "ReceiptHandle": receipt, "Body": m.body, "Attributes": attributes, "MessageAttributes": message_attributes }));
                    }
                    if given.is_empty() && Instant::now() < until {
                        drop(queues);
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                    return ok(if given.is_empty() { serde_json::json!({}) } else { serde_json::json!({ "Messages": given }) });
                }
                "DeleteMessage" | "ChangeMessageVisibility" => {
                    let receipt = json["ReceiptHandle"].as_str().unwrap_or_default();
                    let Some(at) = queue.iter().position(|m| m.receipt.as_deref() == Some(receipt)) else {
                        return refusal(400, "ReceiptHandleIsInvalid", "The input receipt handle is invalid.");
                    };
                    if action == "DeleteMessage" {
                        queue.remove(at);
                    } else {
                        queue[at].visible_at = Instant::now() + Duration::from_secs(json["VisibilityTimeout"].as_u64().unwrap_or(0));
                    }
                    return ok(serde_json::json!({}));
                }
                _ => return refusal(400, "InvalidAction", "The action or operation requested is invalid."),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queues_url_names_where_it_is_its_region_and_its_keys() {
        let q = Queue::parse("https://AKIAX:abc%2Fdef%2B@sqs.eu-west-1.amazonaws.com/123456789012/payouts").unwrap();
        assert_eq!((q.address(), q.region.as_str(), q.keys.id.as_str(), q.keys.secret.as_str()), ("https://sqs.eu-west-1.amazonaws.com/123456789012/payouts".into(), "eu-west-1", "AKIAX", "abc/def+"));
        assert_eq!(Queue::parse("https://k:s@eu-west-2.queue.amazonaws.com/1/q").unwrap().region, "eu-west-2");
        assert_eq!(Queue::parse("https://k:s@sqs-fips.us-east-1.amazonaws.com:443/1/q.fifo").unwrap().address(), "https://sqs-fips.us-east-1.amazonaws.com/1/q.fifo");
        let local = Queue::parse("http://k:s@127.0.0.1:9324/000000000000/payouts?region=elasticmq&token=t0k").unwrap();
        assert_eq!((local.region.as_str(), local.host.as_str(), local.keys.token.as_deref().map(|t| t.as_str())), ("elasticmq", "127.0.0.1:9324", Some("t0k")));
        let refused = |url: &str| Queue::parse(url).err().unwrap();
        assert!(refused("https://sqs.eu-west-1.amazonaws.com/1/q").contains("logs in with its access key"));
        assert!(refused("http://k:s@sqs.eu-west-1.amazonaws.com/1/q").contains("only to this machine"));
        assert!(refused("https://k:s@queue.example.com/1/q").contains("?region="));
        assert!(refused("https://k:s@sqs.eu-west-1.amazonaws.com/").contains("the queue's own"));
        assert!(refused("https://k:s@sqs.eu-west-1.amazonaws.com/1/q?visibility=3").contains("it takes region and token"));
    }

    #[test]
    fn the_stand_in_answers_what_its_queues_hold_and_refuses_a_wrong_signature() {
        let sqs = stand_in::StandIn::start();
        sqs.make("payouts");
        sqs.make("retries.fifo");
        let q = Queue::parse(&sqs.url("payouts")).unwrap();
        assert_eq!(q.attributes().unwrap(), Attributes { fifo: false, largest: 262_144 });
        let id = q.send("{\"n\":1}", None, None).unwrap();
        let got = q.receive(10, 0, 30).unwrap();
        assert_eq!((got.len(), got[0].id.as_str(), got[0].body.as_str(), got[0].count), (1, id.as_str(), "{\"n\":1}", 1));
        assert!(q.receive(10, 0, 30).unwrap().is_empty(), "out of sight");
        q.extend(&got[0].receipt, 0).unwrap();
        let again = q.receive(10, 0, 30).unwrap();
        assert_eq!(again[0].count, 2);
        q.delete(&again[0].receipt).unwrap();
        assert!(sqs.messages("payouts").is_empty());

        let fifo = Queue::parse(&sqs.url("retries.fifo")).unwrap();
        assert!(fifo.attributes().unwrap().fifo);
        let one = fifo.send("a", Some("acct-1"), Some("run-1")).unwrap();
        assert_eq!(fifo.send("a", Some("acct-1"), Some("run-1")).unwrap(), one, "deduplicated");
        assert!(matches!(fifo.send("b", None, None), Err(Failure::Permanent(e)) if e.contains("MissingParameter")));

        let wrong = Queue::parse(&sqs.url("payouts").replace("%2B", "%2C")).unwrap();
        assert!(matches!(wrong.attributes(), Err(Failure::Permanent(e)) if e.contains("403") && e.contains("SignatureDoesNotMatch")));
        let missing = Queue::parse(&sqs.url("nowhere")).unwrap();
        assert!(matches!(missing.attributes(), Err(Failure::Permanent(e)) if e.contains("QueueDoesNotExist")));
        sqs.fail(&[(500, "InternalError"), (400, "RequestThrottled")]);
        assert!(matches!(q.send("x", None, None), Err(Failure::Temporary(_))));
        assert!(matches!(q.send("x", None, None), Err(Failure::Temporary(_))));
    }

    /// A refusal in JSON, as SQS answers its JSON protocol, and in the query protocol's XML, as
    /// moto answers a key it doesn't take: each says what it is.
    #[test]
    fn a_refusal_says_what_it_is_in_json_or_in_xml() {
        let json = r#"{"__type":"com.amazonaws.sqs#QueueDoesNotExist","message":"The specified queue does not exist."}"#;
        let xml = "<?xml version=\"1.0\"?><ErrorResponse><Errors><Error><Type>Sender</Type><Code>SignatureDoesNotMatch</Code><Message>The request signature we calculated does not match the signature you provided.</Message></Error></Errors></ErrorResponse>";
        let text = |f: Failure| match f {
            Failure::Permanent(s) | Failure::Temporary(s) => s,
        };
        assert_eq!(text(said("q", "GetQueueAttributes", 400, json)), "q answered 400 to GetQueueAttributes: QueueDoesNotExist: The specified queue does not exist.");
        assert_eq!(text(said("q", "GetQueueAttributes", 403, xml)), "q answered 403 to GetQueueAttributes: SignatureDoesNotMatch: The request signature we calculated does not match the signature you provided.");
        assert_eq!(text(said("q", "SendMessage", 403, "")), "q answered 403 to SendMessage");
        assert!(matches!(said("q", "SendMessage", 400, "<Code>ThrottlingException</Code>"), Failure::Temporary(_)), "a throttle in XML too");
        assert!(matches!(said("q", "SendMessage", 503, "<html>busy</html>"), Failure::Temporary(_)));
    }
}
