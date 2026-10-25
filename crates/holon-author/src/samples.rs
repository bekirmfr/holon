//! Sample flows: one per capability, opened from the editor's flows page as an unsaved copy (and
//! written by `holon new --sample`). Each is a `.holon` file in `samples/`, written in the
//! language (docs/LANGUAGE.md) in the canonical form `holon fmt` writes, embedded here and compiled
//! through the same lowering as any text, against the live catalog, so a sample can't drift from
//! core: a sample that stops compiling fails the tests (here, and the editor's samples.test.ts,
//! which builds every one and derives it through the real core). Every new feature ships with a
//! sample that shows it (CLAUDE.md); the sample is the feature's documentation, readable as is.

use serde::Serialize;

use crate::builder::FlowDocument;
use crate::lang;

/// A secret a sample needs set under ⚙ before it works.
#[derive(Serialize, Clone, Debug)]
pub struct SampleSecret {
    pub name: &'static str,
    /// What it is.
    pub what: &'static str,
    /// Where to keep it when not a value sealed by the server (`env:HOLON_SECRET_…`, `vault:…`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<&'static str>,
}

/// A sample: what the flows page shows of it, and the flow itself, as text.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    /// The flow's name when opened (and its deployment's slug once deployed).
    pub name: &'static str,
    pub title: &'static str,
    /// One line: what it demonstrates.
    pub description: &'static str,
    /// The features it shows, as the palette and docs name them.
    pub shows: &'static [&'static str],
    /// Deploy settings: a server's port, or a clock flow's interval in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<&'static str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub secrets: &'static [SampleSecret],
    /// Permissions it can run without (deploy settings' `optional`); the rest are required.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub optional: &'static [&'static str],
    /// Web pages from other sites it answers (deploy settings' `browsers`: `*`, or origins).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browsers: Option<&'static str>,
    /// How to try it once deployed.
    pub try_it: &'static str,
    /// The flow, in the language (docs/LANGUAGE.md): the file beside this crate's `src`, in
    /// `samples/`, in the canonical form `holon fmt` writes. `holon new --sample` and the flows
    /// page's download write it as it is, its names and all.
    pub text: &'static str,
}

/// Every sample, in the order the flows page lists them.
pub fn samples() -> &'static [Sample] {
    SAMPLES
}

/// The sample named `name`.
pub fn sample(name: &str) -> Option<&'static Sample> {
    SAMPLES.iter().find(|s| s.name == name)
}

/// Lay out `sample` on a canvas: the flow document opening it loads, its holons' units (the whole
/// closure) in it, as a saved flow's are. Its text, lowered, and carried as the document's
/// `source`, so the text pane opens on it and a deploy keeps its names (`explain` reads a run as
/// `key = req.headers[...]`, not `lookup_2 = …`).
pub fn build(sample: &Sample) -> Result<FlowDocument, String> {
    let mut doc = lang::compile(sample.text).map(|l| l.doc).map_err(|e| format!("sample \"{}\": {e}", sample.name))?;
    doc.extra.insert("source".into(), serde_json::Value::String(sample.text.to_string()));
    Ok(doc)
}

const RECEIPT_KEY: &str = r#"SecretRead("receipt_key")"#;

static SAMPLES: &[Sample] = &[
    Sample {
        name: "hello-api",
        title: "Hello API",
        description: "A route with a path parameter, answered with text: the smallest server.",
        shows: &["request", "respond", "path params", "concat"],
        port: Some("8101"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "curl http://127.0.0.1:8101/hello/Ada",
        text: include_str!("../samples/hello-api.holon"),
    },
    Sample {
        name: "notes-api",
        title: "Notes API",
        description: "Create, read, list (a page at a time) and delete notes kept in a collection, keyed by id.",
        shows: &["collections (get, list, put, delete)", "paging with after", "JSON bodies", "404 and 204", "tracing a failed run"],
        port: Some("8102"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X PUT -H "Content-Type: application/json" -d '{"text":"buy milk"}' http://127.0.0.1:8102/notes/1, then GET /notes (?after=1 for the next page), GET /notes/1, DELETE /notes/1. GET /notes/9 fails (404): open the run in view mode to see where, and why"#,
        text: include_str!("../samples/notes-api.holon"),
    },
    Sample {
        name: "stripe-to-slack",
        title: "Stripe → Slack",
        description: "Verify a Stripe webhook's signature and post the event to a Slack channel; anything unsigned gets 400 and posts nothing.",
        shows: &["stripe_verify", "secrets", "http_request", "clock", "hmac_sha256", "split"],
        port: Some("8103"),
        interval: None,
        secrets: &[
            SampleSecret { name: "stripe_webhook_secret", what: "the endpoint's signing secret from Stripe (whsec_…)", from: None },
            SampleSecret { name: "slack_webhook_path", what: "your Slack incoming webhook's path (/services/…)", from: None },
        ],
        optional: &[],
        browsers: None,
        try_it: "stripe listen --forward-to 127.0.0.1:8103/stripe, then stripe trigger payment_intent.succeeded",
        text: include_str!("../samples/stripe-to-slack.holon"),
    },
    Sample {
        name: "guarded-api",
        title: "Token-guarded API",
        description: "Answer only callers holding the API token, with a receipt signed by a key from the server's environment: secrets sealed by the server or kept elsewhere, versioned, rolled back; the receipt key is an optional permission.",
        shows: &["secrets", "secret sources", "secret versions", "optional permissions", "secure_equals", "guard", "hmac_sha256", "default"],
        port: Some("8109"),
        interval: None,
        secrets: &[
            SampleSecret { name: "api_token", what: "the bearer token callers send; set a new one to rotate it, roll back under ⚙ → ▸", from: None },
            SampleSecret { name: "receipt_key", what: "the key receipts are signed with", from: Some("env:HOLON_SECRET_RECEIPT_KEY") },
        ],
        // Without the receipt key it still answers, the receipt "unsigned": revoking it doesn't pause it.
        optional: &[RECEIPT_KEY],
        browsers: None,
        try_it: r#"curl -H "Authorization: Bearer <api_token>" http://127.0.0.1:8117/me answers a receipt; without it, or after the token is rotated, 401; revoke receipt_key and it answers "unsigned""#,
        text: include_str!("../samples/guarded-api.holon"),
    },
    Sample {
        name: "slugs",
        title: "A flow inside a flow (holon)",
        description: "Two holons, each a whole flow placed as one node: slugify (lowercase, split, join with -) only computes, placed twice; publish reads and writes outside (the clock, a collection, state, an optional secret) and its manifest, permissions, replay and flow state count all of it. A saved flow without routes is placed the same way, from the palette's flows.",
        shows: &["holons", "group / expand", "saved flows as nodes", "composite reuse", "holons with effects", "optional permissions"],
        port: Some("8110"),
        interval: None,
        secrets: &[SampleSecret { name: "receipt_key", what: r#"signs each publish receipt; optional: without it receipts read "unsigned""#, from: None }],
        optional: &[RECEIPT_KEY],
        browsers: None,
        try_it: r#"curl -X POST -H "Content-Type: application/json" -d '{"title": "Hello Holon World", "author": "Ada Lovelace"}' http://127.0.0.1:8110/slugs answers the two slugs and a receipt; GET /slugs lists what was published; view a run to see the holons replayed and the post written, and expand a holon to see inside"#,
        text: include_str!("../samples/slugs.holon"),
    },
    Sample {
        name: "issue-links",
        title: "Issue links",
        description: "Every link to one issue, built from its path with thirteen interpolated parts in one route: the same node many times over, each told apart by where its value goes.",
        shows: &["interpolation", "path params", "records"],
        port: Some("8111"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "curl http://127.0.0.1:8111/repos/bekirmfr/holon/issues/7",
        text: include_str!("../samples/issue-links.holon"),
    },
    Sample {
        name: "quote",
        title: "Quote (functions that use what's around them)",
        description: "Prices a list of items with the request's tax: each line's function uses tax from the route around it, which reaches the function as an input of its own, and a fold sums the lines.",
        shows: &["map", "fold", "lambdas", "captures"],
        port: Some("8112"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X POST -H "Content-Type: application/json" -d '{"tax":0.25,"items":[{"name":"tea","price":4,"qty":2},{"name":"cake","price":3,"qty":1}]}' http://127.0.0.1:8112/quote"#,
        text: include_str!("../samples/quote.holon"),
    },
    Sample {
        name: "basket",
        title: "Basket (a collection read inside a function)",
        description: "Prices a basket from a price list kept in a collection: each line's function reads its item's price, so the read happens once per item, each answered on its own, recorded and replayed. An item with no price answers 404 instead of a made-up total.",
        shows: &["map", "fold", "collections", "reads inside a function"],
        port: Some("8113"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X PUT -d '{"price":4}' http://127.0.0.1:8113/prices/tea and again for cake at 3, then curl -X POST -d '{"items":[{"name":"tea","qty":2},{"name":"cake","qty":1}]}' http://127.0.0.1:8113/basket (an unpriced item: 404)"#,
        text: include_str!("../samples/basket.holon"),
    },
    Sample {
        name: "tickets",
        title: "Tickets (counting to n)",
        description: "Hands out count numbered tickets for an event: range counts from 0 to count-1, and a function names each one, using the event from around it. range stops at 10000, so a map over it always ends; a count it can't count to answers 400.",
        shows: &["range", "map", "lambdas", "interpolation"],
        port: Some("8114"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X POST -d '{"event":"gig","count":3}' http://127.0.0.1:8114/tickets (a count of 2.5, -1 or 20000: 400)"#,
        text: include_str!("../samples/tickets.holon"),
    },
    Sample {
        name: "cache-aside",
        title: "Cache-aside (a branch not taken)",
        description: "Hands out an id per name, fetched once (a fresh uuid from httpbin) and kept in a collection: a name it has an id for is answered from the collection, and the fetch, the ?? fallback it doesn't need, isn't run at all, so no request goes out. The run log and view mode show it as not run.",
        shows: &["coalesce", "lazy choices", "collections", "http"],
        port: Some("8115"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl http://127.0.0.1:8115/id/ada twice: the same id, and the second run sends no request (or set one: curl -X PUT -d '{"id":"x"}' http://127.0.0.1:8115/id/bob)"#,
        text: include_str!("../samples/cache-aside.holon"),
    },
    Sample {
        name: "calc",
        title: "Calculator (a match)",
        description: "Answers a + b, a - b, a * b or a / b by the op it is sent: a match chooses one arm, the others don't run, and an arm chosen that has no value (a division by zero) answers 400 instead of some other arm's value.",
        shows: &["match", "lazy choices", "missing"],
        port: Some("8116"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -d '{"op":"add","a":1,"b":0}' http://127.0.0.1:8116/calc answers 1; with "op":"div" it answers 400"#,
        text: include_str!("../samples/calc.holon"),
    },
    Sample {
        name: "heartbeat",
        title: "Heartbeat",
        description: "A clock flow: every 10 seconds it records the time, counts the beats in its state, and keeps the time of the first beat (a fallback that is itself a value: coalesce).",
        shows: &["clock", "interval", "state across runs", "default", "coalesce", "add"],
        port: None,
        interval: Some("10"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "deploy it, wait, then look at its state on the deployed page (or GET /deployments/heartbeat/state)",
        text: include_str!("../samples/heartbeat.holon"),
    },
    Sample {
        name: "approval",
        title: "Order approval (durable run)",
        description: "An order waits for payment: the run hands out a pay link, parks, and finishes when the link is opened, hours or days later. Its progress goes out on the channel orders as it happens: the pack that doesn't wait for the payment as the run parks, the one that does when it finishes, never the first twice.",
        shows: &["await", "resume_token", "resume routes", "durable runs", "channels", "progress from a waiting run"],
        port: Some("8105"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X POST -H "Content-Type: application/json" -d '{"item":"book"}' http://127.0.0.1:8105/orders (answers /paid/<token>), then open that path; the order is written to state "last_order". curl -N http://127.0.0.1:8105/orders/live shows each order's progress: "waiting for payment", then "paid""#,
        text: include_str!("../samples/approval.holon"),
    },
    Sample {
        name: "fetch-json",
        title: "Fetch and reshape",
        description: "Call another API, parse its JSON and answer with the one field you need.",
        shows: &["http_request", "json_parse", "field", "host permissions"],
        port: Some("8106"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "curl http://127.0.0.1:8106/slideshow (calls https://httpbin.org/json, answers its title)",
        text: include_str!("../samples/fetch-json.holon"),
    },
    Sample {
        name: "sign-payload",
        title: "Sign a payload",
        description: "Sign whatever is posted with a secret key (HMAC-SHA256) and answer the signature with the body in base64.",
        shows: &["hmac_sha256", "base64", "secrets", "record building"],
        port: Some("8107"),
        interval: None,
        secrets: &[SampleSecret { name: "signing_key", what: "any secret of 6 characters or more to sign with", from: None }],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X POST -H "Content-Type: application/json" -d 'hello' http://127.0.0.1:8107/sign"#,
        text: include_str!("../samples/sign-payload.holon"),
    },
    Sample {
        name: "likes",
        title: "Likes counter (runs side by side)",
        description: "Each like reads a post's count from a collection, adds one and writes it back. Likes arriving at once run side by side; one whose count changed under it runs again, so none is lost.",
        shows: &["concurrent runs", "retries", "read-modify-write on a collection item", "collection_list", "as_text"],
        port: Some("8108"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "seq 50 | xargs -P 20 -I{} curl -s -X POST http://127.0.0.1:8108/posts/1/like, then GET /posts/1/likes (\"50 likes\") and GET /likes; the log marks runs that ran again",
        text: include_str!("../samples/likes.holon"),
    },
    Sample {
        name: "witness",
        title: "Run witness",
        description: "Takes the run heads a holon server sends it (holon serve --export-heads), with the witness key: keeps each batch with when it arrived and answers a receipt, so a run log changed afterwards no longer matches what was witnessed.",
        shows: &["run log hash chain", "--export-heads", "collections", "secrets", "secure_equals", "guard"],
        port: Some("8130"),
        interval: None,
        secrets: &[SampleSecret { name: "witness_key", what: "the token servers send (their HOLON_EXPORT_HEADS_TOKEN)", from: None }],
        optional: &[],
        browsers: None,
        try_it: "start a server with HOLON_EXPORT_HEADS_TOKEN set to the witness key and --export-heads http://127.0.0.1:8130/heads, let any deployment run, then GET http://127.0.0.1:8130/batches",
        text: include_str!("../samples/witness.holon"),
    },
    Sample {
        name: "ledger",
        title: "Ledger (every run checkable)",
        description: "Keeps a balance per account: each post adds its amount and answers the new balance with when it was posted. Export its runs and holon verify runs every posting again, offline, from what its log entry recorded (the balance it read, the clock), and checks it produced what the log says.",
        shows: &["holon verify", "run export", "collections", "clock", "with_field"],
        port: Some("8131"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X POST -d '{"amount":5}' http://127.0.0.1:8131/accounts/ada a few times, save GET /deployments/<slug>/export (signed in) as runs.json, then holon verify runs.json"#,
        text: include_str!("../samples/ledger.holon"),
    },
    Sample {
        name: "marketplace",
        title: "Marketplace (built with Holon)",
        description: "A marketplace for holons, itself a Holon flow: publishing a listing (a holon's name, description, ports, manifest and its units, by hash) takes the publish key; browsing and reading one are open to any web page; a bad version is yanked, never edited. The editor publishes saved flows to it and places what it lists, checking every unit against its hash.",
        shows: &["marketplace", "web pages from other sites (CORS)", "collections", "secrets", "secure_equals", "guard", "with_field"],
        port: Some("8120"),
        interval: None,
        secrets: &[SampleSecret { name: "publish_key", what: "the key publishers send (Authorization: Bearer <key>) to publish or yank", from: None }],
        optional: &[],
        browsers: Some("*"),
        try_it: "set it as the editor's marketplace (flows page), publish a saved flow without routes from the flows page, and place it from the palette's marketplace; GET http://127.0.0.1:8120/listings lists them",
        text: include_str!("../samples/marketplace.holon"),
    },
    Sample {
        name: "nearest",
        title: "Nearest word (a js node)",
        description: "Keep a dictionary of words and answer the closest one to a query by edit distance: the one dense computation is a js node, JavaScript run as a module with no imports under a fuel budget; the flow around it stores, routes and answers.",
        shows: &["js node", "module", "collections", "keys", "field"],
        port: Some("8117"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "curl -X POST --data-binary kitten http://127.0.0.1:8117/words, then sitting and mitten; curl http://127.0.0.1:8117/nearest/kittne answers { best: \"kitten\", distance: 2 }; a word nobody added yet answers 404",
        text: include_str!("../samples/nearest.holon"),
    },
    Sample {
        name: "tally",
        title: "Tally a CSV (a js node parsing text)",
        description: "Post a CSV and get a glance at each column: the kind, min, max and mean of a number column, the distinct values and the top three of a text column. Parsing text into structure is a js node's work; the flow stores the last summary in its state and answers it.",
        shows: &["js node", "state", "multi-output", "records and lists out of JavaScript"],
        port: Some("8119"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "printf 'city,temp\nOslo,4\nRome,19\nOslo,6\n' | curl -s -X POST --data-binary @- http://127.0.0.1:8119/tally answers rows 3 and two columns (city: text, 2 distinct; temp: number, 4 to 19, mean 9.67); curl http://127.0.0.1:8119/tally/last repeats it with when",
        text: include_str!("../samples/tally.holon"),
    },
    Sample {
        name: "idempotent",
        title: "Idempotent charges (one call per key)",
        description: "Charge once per Idempotency-Key: the same key with the same body answers the first charge again, the same key with a different body is refused (409), a key expires after a day, and the processor is called only when there is no prior charge. Two identical requests at once: the second waits for the first's claim on the key, reads what it wrote, and sends nothing (docs/L4s/key-claims-L4.md).",
        shows: &["idempotency key", "key claims", "lazy ??", "sha256 fingerprint", "collection", "absence as refusal"],
        port: Some("8121"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "curl -X POST -H 'Idempotency-Key: k1' -H 'Content-Type: application/json' -d '{\"amount\":42}' http://127.0.0.1:8121/charges twice: the same id, the second replayed: true; send it with amount 43 for a 409; run two at once with a fresh key and `explain` shows the second claimed charges[k2], waited, and sent nothing",
        text: include_str!("../samples/idempotent.holon"),
    },
    Sample {
        name: "guestbook",
        title: "Guestbook (a page from a flow)",
        description: "A web page served by the flow itself: a js node renders the guestbook as HTML from the entries in a collection, with the clock wired in to say how long ago each was signed; another parses the posted form; the flow stores a signature and redirects back to the page. The response is an envelope: status, headers (content-type: text/html) and body.",
        shows: &["js node", "HTML response (an envelope with headers)", "forms", "collections", "redirect (303)", "the clock wired into code"],
        port: Some("8122"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "open http://127.0.0.1:8122/ in a browser and sign it; or curl -s -d 'name=Ada&note=Hello' http://127.0.0.1:8122/sign -o /dev/null -w '%{http_code}' answers 303, and curl http://127.0.0.1:8122/ shows the entry",
        text: include_str!("../samples/guestbook.holon"),
    },
    Sample {
        name: "inbox-report",
        title: "Inbox report (files in folders on the host)",
        description: "Reads a CSV from a folder on the host, counts its rows and totals its number columns in a js node, and writes a Markdown report to another folder; lists and serves the reports. The flow names its folders (inbox, reports), never a path: an admin binds each to a real folder, read-only or read-write, and deploying asks for each use. A write lands whole when the run finishes; the run log keeps what was read and a hash of what was written.",
        shows: &["named folders", "file reads and writes", "js node", "listing files"],
        port: Some("8123"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folders (an admin): holon folder add inbox ~/holon-inbox --read-only and holon folder add reports ~/holon-reports; put sales.csv (region,amount / north,10 / south,32) in the inbox; then curl -s -X POST -H 'content-type: application/json' -d '{\"name\":\"sales.csv\"}' http://127.0.0.1:8123/report answers rows 2, curl http://127.0.0.1:8123/reports/sales.csv.md shows the report (amount: 42), and curl http://127.0.0.1:8123/reports lists it",
        text: include_str!("../samples/inbox-report.holon"),
    },
    Sample {
        name: "log-archive",
        title: "Log archive (big files, by handle and by range)",
        description: "Serves the last lines and the first bytes of a log of any size, and archives a log to another folder host to host. Only the lines or bytes asked for enter the flow, and the run log keeps with them the hashes that prove them part of the file, so holon verify checks the run without the file. Archiving reads a handle (name, size, hash), never the bytes: the copy streams them when the run finishes, and fails if the log changed since it was read.",
        shows: &["named folders", "file handles", "copy by handle", "lines and slices", "range proofs"],
        port: Some("8124"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folders (an admin): holon folder add logs ~/holon-logs --read-only and holon folder add archive ~/holon-archive; put any app.log in the logs folder; then curl http://127.0.0.1:8124/logs/app.log/tail answers its last 20 lines, curl http://127.0.0.1:8124/logs/app.log/head its first 256 bytes, curl -s -X POST -H 'content-type: application/json' -d '{\"name\":\"app.log\"}' http://127.0.0.1:8124/archive copies it into the archive under its hash, and curl http://127.0.0.1:8124/archive lists the archive",
        text: include_str!("../samples/log-archive.holon"),
    },
    Sample {
        name: "journal",
        title: "Journal (a folder kept for a while, and capped)",
        description: "Appends each line posted to a page, a text file in a folder on the host, and serves the page. Every run reads the page whole, so the run log keeps what it read: past 4 KiB in the blob store, by content and in chunks, so a page that grows by a line adds a chunk, not a copy. The admin decides how long those texts are kept (keep: bytes for 7d, then only their hash) and how big the folder may get (a cap: a write past it doesn't land, and the run answers 500 saying so).",
        shows: &["named folders", "read then write", "kept for a while", "folder caps"],
        port: Some("8125"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folder (an admin): holon folder add journal ~/holon-journal --keep 7d --cap 10M; then curl -s -X POST -d 'first line' http://127.0.0.1:8125/journal/today and curl -s -X POST -d 'second line' http://127.0.0.1:8125/journal/today append to today.txt, curl http://127.0.0.1:8125/journal/today shows both lines, and curl http://127.0.0.1:8125/journal lists the pages; holon folder sweep removes now the texts no run kept within 7 days refers to",
        text: include_str!("../samples/journal.holon"),
    },
    Sample {
        name: "live-board",
        title: "Live board (a channel: packs to whoever listens)",
        description: "A page that updates itself: posting a ticket stores it and sends a pack on the channel updates; every page open on the board is subscribed (GET /live answers with the channel's packs as server-sent events), and its EventSource adds the ticket as it arrives. A pack goes out once its run is over and its writes have landed, as the run is logged; a page that reconnects gets the packs it missed. Sending and letting callers subscribe are each a permission.",
        shows: &["channels", "server-sent events", "js node", "HTML response (an envelope with headers)", "collections"],
        port: Some("8126"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "open http://127.0.0.1:8126/ in two browsers and add a ticket in one: it shows in both; or curl -N http://127.0.0.1:8126/live in one terminal and curl -s -H 'content-type: application/json' -d '{\"title\":\"fix the door\"}' http://127.0.0.1:8126/tickets in another: the first prints id: … and data: {\"at\":…,\"title\":\"fix the door\"}",
        text: include_str!("../samples/live-board.holon"),
    },
    Sample {
        name: "chat-proxy",
        title: "Chat proxy (a streamed reply)",
        description: "Asks Claude (the Messages API, streamed) what is posted, and forwards the answer to the caller as it arrives: the call's `stream: true` and the route's `respond sent.body, stream: true` say so, and the page shows the words as they come. The flow still gets the whole answer once it ends, and stores the prompt with what was said (a js node reads the text out of the events). POST /room pipes the answer to the channel room instead (`room.pipe(http.post(…))`): whoever watches GET /room/live gets its pieces as they arrive, then the whole of it once the run is over. The key is a secret; an answer has the run's 25 seconds (run_budget_ms under ⚙ for longer ones).",
        shows: &["streamed replies", "http_stream", "pipe", "channels", "server-sent events", "secrets", "js node", "collections"],
        port: Some("8127"),
        interval: None,
        secrets: &[SampleSecret { name: "anthropic_api_key", what: "an Anthropic API key (sk-ant-…), from console.anthropic.com", from: None }],
        optional: &[],
        browsers: None,
        try_it: "open http://127.0.0.1:8127/ and ask something: the answer shows as it is written; or curl -N -H 'content-type: application/json' -d '{\"prompt\":\"a haiku about rivers\"}' http://127.0.0.1:8127/chat prints the events as they come. GET / lists what was asked before. For the room: curl -N http://127.0.0.1:8127/room/live in one terminal, then curl -s -H 'content-type: application/json' -d '{\"prompt\":\"a haiku about rivers\"}' http://127.0.0.1:8127/room in another: the first prints the pieces as they come, then {\"prompt\":…,\"said\":…}",
        text: include_str!("../samples/chat-proxy.holon"),
    },
    Sample {
        name: "drop-box",
        title: "Drop box (a run per new file, sent on by handle)",
        description: "Runs once for each new file dropped into a folder on the host (on file drop), when it stops changing: the run's ingress is the file's handle (name, size, hash). It sends the file to a server as the body of a PUT, by handle: the server streams the bytes from the folder, never through the flow, and the run log keeps only their hash. It writes a receipt for each file to another folder. Files already there when it is deployed aren't new; one it writes into a folder it watches doesn't start it again.",
        shows: &["on file", "file handles", "a file as a request body", "named folders"],
        port: None,
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folders (an admin): holon folder add drop ~/holon-drop --read-only and holon folder add receipts ~/holon-receipts; deploy, then copy any file into ~/holon-drop: within two seconds ~/holon-receipts holds <file>.json with its size, its hash and how many bytes httpbin.org received, and the deployment's log shows the run",
        text: include_str!("../samples/drop-box.holon"),
    },
    Sample {
        name: "file-audit",
        title: "File audit (runs on several folders, removals and some names)",
        description: "Keeps an audit trail of two folders on the host, one entry per change: a CSV new or changed in the inbox (on file inbox matching \"*.csv\"), any file removed from it (on file inbox deleted), and any file new or changed in the outbox. The run's ingress is the file's handle with its event (new, changed or deleted; a removed file as last seen, without a hash), whichever line started it. Each entry is created, never replaced (audit.create): a write that would replace one fails the run.",
        shows: &["on file", "several on file lines", "on file deleted", "matching", "create-only writes"],
        port: None,
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folders (an admin): holon folder add inbox ~/holon-inbox --read-only, holon folder add outbox ~/holon-outbox --read-only and holon folder add audit ~/holon-audit; deploy, then copy a .csv into ~/holon-inbox, delete a file from it, or drop any file into ~/holon-outbox: within two seconds ~/holon-audit/<folder>/ holds an entry for each change; a .txt dropped into the inbox starts nothing",
        text: include_str!("../samples/file-audit.holon"),
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composites::Names;
    use holon_core::MapResolver;
    use std::collections::HashSet;

    #[test]
    fn every_sample_builds_with_a_unique_name_and_port() {
        let names: HashSet<_> = samples().iter().map(|s| s.name).collect();
        assert_eq!(names.len(), samples().len());
        let ports: Vec<_> = samples().iter().filter_map(|s| s.port).collect();
        assert_eq!(ports.iter().collect::<HashSet<_>>().len(), ports.len(), "each server its own port");
        for s in samples() {
            // What starts it: requests on a port, the clock, or new files (`on file`).
            let on_file = s.text.lines().any(|l| l.starts_with("on file "));
            assert_eq!([s.port.is_some(), s.interval.is_some(), on_file].iter().filter(|b| **b).count(), 1, "{}: a port, an interval or on file", s.name);
            let doc = build(s).unwrap_or_else(|e| panic!("{e}"));
            assert!(!doc.nodes.is_empty(), "{}", s.name);
        }
    }

    /// A sample's document carries its text as `source`: the text pane opens on it, and a deploy
    /// from the editor keeps the text's names.
    #[test]
    fn a_samples_document_carries_its_text() {
        for s in samples() {
            let doc = build(s).unwrap();
            assert_eq!(doc.extra.get("source").and_then(serde_json::Value::as_str), Some(s.text), "{}", s.name);
        }
    }

    /// The table says what the flows page shows; the text says what deploys. They agree.
    #[test]
    fn a_samples_text_says_what_its_entry_says() {
        for s in samples() {
            let lowered = lang::compile(s.text).unwrap_or_else(|e| panic!("{}: {e}", s.name));
            assert_eq!(lowered.name.as_deref(), Some(s.name), "{}: flow name", s.name);
            let server = lowered.doc.server.unwrap_or_default();
            assert_eq!(server.port, s.port.unwrap_or_default(), "{}: port", s.name);
            assert_eq!(server.interval, s.interval.unwrap_or_default(), "{}: interval", s.name);
            assert_eq!(server.browsers.as_deref(), s.browsers, "{}: browsers", s.name);
            let optional: Vec<String> = server.requirements.and_then(|r| r.optional).unwrap_or_default();
            assert_eq!(optional, s.optional, "{}: optional", s.name);
            for secret in s.secrets {
                assert!(s.text.contains(&format!("secret.{}", secret.name)), "{}: reads secret.{}", s.name, secret.name);
            }
        }
    }

    /// Each file is in the canonical form: `holon fmt` would leave it as it is.
    #[test]
    fn every_sample_is_as_fmt_writes_it() {
        for s in samples() {
            let lowered = lang::compile(s.text).unwrap_or_else(|e| panic!("{}: {e}", s.name));
            let printed = lang::print(lowered.name.as_deref(), &lowered.doc, &mut MapResolver::new(), &Names::new(), &lowered.names).unwrap();
            assert_eq!(printed, s.text, "{}: samples/{}.holon isn't as fmt writes it", s.name, s.name);
        }
    }

    #[test]
    fn a_mistake_in_a_sample_names_it_and_the_line() {
        let broken = Sample { text: "flow hello-api\nport 8101\n\nroute GET /hello/:name (req) {\n  respond concat(\"Hello, \", req.params.nmae.x, nope)\n}\n", ..sample("hello-api").unwrap().clone() };
        assert_eq!(build(&broken).unwrap_err(), r#"sample "hello-api": 5:48: unknown name "nope""#);
    }

    /// Lazy choices change which nodes run, never a value (docs/L4s/lazy-choices-L4.md §2): every
    /// sample's routes, sent a request with each path parameter and a JSON body, give what
    /// evaluating every node gives.
    #[test]
    fn every_samples_routes_give_what_the_eager_evaluator_gives() {
        use holon_core::{eager::eval_eager, eval, parse_hash, Resolver, Value};
        use std::collections::BTreeMap;
        let text = |s: &str| Value::Text(s.into());
        let mut checked = 0;
        for s in samples() {
            let lowered = lang::compile(s.text).unwrap_or_else(|e| panic!("{}: {e}", s.name));
            let mut store = MapResolver::new();
            crate::text::restore(&lowered.doc, &mut store, &Names::new());
            let port = lowered.doc.server.as_ref().map(|x| x.port.clone()).unwrap_or_default();
            let server = crate::flow::derive_server(&lowered.doc.nodes, &lowered.doc.edges, &mut store, &port);
            for e in &server.endpoints {
                let unit = store.resolve(&parse_hash(&e.hash).unwrap()).unwrap();
                let params: BTreeMap<String, Value> = e.path.split('/').filter_map(|seg| seg.strip_prefix(':')).map(|k| (k.to_string(), text("tea"))).collect();
                for body in ["", r#"{"id":"x","price":4,"amount":5,"items":[{"name":"tea","qty":2}],"event":"gig","count":2,"text":"hi"}"#] {
                    let request = Value::Record(BTreeMap::from([
                        ("method".to_string(), text(&e.method)),
                        ("path".to_string(), text(&e.path)),
                        ("params".to_string(), Value::Record(params.clone())),
                        ("query".to_string(), Value::Record(BTreeMap::new())),
                        ("headers".to_string(), Value::Record(BTreeMap::new())),
                        ("body".to_string(), text(body)),
                    ]));
                    let inputs = [Some(request)];
                    assert_eq!(eval(unit, &inputs, &store).into_vec(), eval_eager(unit, &inputs, &store), "{} {} {}", s.name, e.method, e.path);
                    checked += 1;
                }
            }
        }
        assert!(checked > 40, "{checked}");
    }

    #[test]
    fn slugs_places_one_unit_twice() {
        let doc = build(sample("slugs").unwrap()).unwrap();
        let placed: Vec<_> = doc.nodes.iter().filter(|n| n.data.name == "slugify").collect();
        assert_eq!(placed.len(), 2);
        assert_eq!(placed[0].data.body, placed[1].data.body);
        assert!(!doc.units.unwrap_or_default().is_empty(), "its closure travels with the document");
    }
}
