//! Sample flows: one per capability, opened from the editor's flows page as an unsaved copy (and
//! written by `holon new --sample`). Each is a `.holon.ts` file in `samples/`, written in the
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
    /// Deploy settings: a server's port, for a flow that answers requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<&'static str>,
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
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "curl http://127.0.0.1:8101/hello/Ada",
        text: include_str!("../samples/hello-api.holon.ts"),
    },
    Sample {
        name: "notes-api",
        title: "Notes API",
        description: "Create, read, list (a page at a time) and delete notes kept in a collection, keyed by id.",
        shows: &["collections (get, list, put, delete)", "paging with after", "JSON bodies", "404 and 204", "tracing a failed run"],
        port: Some("8102"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X PUT -H "Content-Type: application/json" -d '{"text":"buy milk"}' http://127.0.0.1:8102/notes/1, then GET /notes (?after=1 for the next page), GET /notes/1, DELETE /notes/1. GET /notes/9 fails (404): open the run in view mode to see where, and why"#,
        text: include_str!("../samples/notes-api.holon.ts"),
    },
    Sample {
        name: "fan-out",
        title: "Webhook fan-out",
        description: "Pass each event on to two listeners and answer 204: a request is an effect, sent whether or not its answer is read, and a clock that only posts is a handler.",
        shows: &["http_request", "requests as effects", "clock"],
        port: Some("8169"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "curl -i -X POST -d '{\"type\":\"paid\"}' http://127.0.0.1:8169/events, then the run log",
        text: include_str!("../samples/fan-out.holon.ts"),
    },
    Sample {
        name: "stripe-to-slack",
        title: "Stripe → Slack",
        description: "Verify a Stripe webhook's signature and post the event to a Slack channel, each half a function, the Slack one async; anything unsigned gets 400 and posts nothing.",
        shows: &["stripe_verify", "secrets", "where secrets go", "http_request", "functions", "async functions", "clock", "hmac_sha256", "split"],
        port: Some("8103"),
        secrets: &[
            SampleSecret { name: "stripe_webhook_secret", what: "the endpoint's signing secret from Stripe (whsec_…)", from: None },
            SampleSecret { name: "slack_webhook_path", what: "your Slack incoming webhook's path (/services/…)", from: None },
        ],
        optional: &[],
        browsers: None,
        try_it: "stripe listen --forward-to 127.0.0.1:8103/stripe, then stripe trigger payment_intent.succeeded",
        text: include_str!("../samples/stripe-to-slack.holon.ts"),
    },
    Sample {
        name: "guarded-api",
        title: "Token-guarded API",
        description: "Answer only callers holding the API token, with a receipt signed by a key from the server's environment: secrets sealed by the server or kept elsewhere, versioned, rolled back; the receipt key is an optional permission.",
        shows: &["secrets", "secret sources", "secret versions", "optional permissions", "secure_equals", "guard", "hmac_sha256", "default"],
        port: Some("8109"),
        secrets: &[
            SampleSecret { name: "api_token", what: "the bearer token callers send; set a new one to rotate it, roll back under ⚙ → ▸ (a dry run says what it would do first, its diff each version's fingerprint)", from: None },
            SampleSecret { name: "receipt_key", what: "the key receipts are signed with", from: Some("env:HOLON_SECRET_RECEIPT_KEY") },
        ],
        // Without the receipt key it still answers, the receipt "unsigned": revoking it doesn't pause it.
        optional: &[RECEIPT_KEY],
        browsers: None,
        try_it: r#"curl -H "Authorization: Bearer <api_token>" http://127.0.0.1:8109/me answers a receipt; without it, or after the token is rotated, 401; revoke receipt_key and it answers "unsigned""#,
        text: include_str!("../samples/guarded-api.holon.ts"),
    },
    Sample {
        name: "slugs",
        title: "A flow inside a flow (holon)",
        description: "Two holons, each a whole flow placed as one node: slugify (lowercase, split, join with -) only computes, placed twice; publish reads and writes outside (the clock, a collection, state, an optional secret) and its manifest, permissions, replay and flow state count all of it. A saved flow without routes is placed the same way, from the palette's flows.",
        shows: &["holons", "group / expand", "saved flows as nodes", "composite reuse", "holons with effects", "optional permissions"],
        port: Some("8110"),
        secrets: &[SampleSecret { name: "receipt_key", what: r#"signs each publish receipt; optional: without it receipts read "unsigned""#, from: None }],
        optional: &[RECEIPT_KEY],
        browsers: None,
        try_it: r#"curl -X POST -H "Content-Type: application/json" -d '{"title": "Hello Holon World", "author": "Ada Lovelace"}' http://127.0.0.1:8110/slugs answers the two slugs and a receipt; GET /slugs lists what was published; view a run to see the holons replayed and the post written, and expand a holon to see inside"#,
        text: include_str!("../samples/slugs.holon.ts"),
    },
    Sample {
        name: "issue-links",
        title: "Issue links",
        description: "Every link to one issue, built from its path with thirteen interpolated parts in one route: the same node many times over, each told apart by where its value goes.",
        shows: &["interpolation", "path params", "records"],
        port: Some("8111"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "curl http://127.0.0.1:8111/repos/bekirmfr/holon/issues/7",
        text: include_str!("../samples/issue-links.holon.ts"),
    },
    Sample {
        name: "quote",
        title: "Quote (functions that use what's around them)",
        description: "Prices a list of items with the request's tax: each line's function uses tax from the route around it, which reaches the function as an input of its own, and a reduce sums the lines.",
        shows: &["map", "fold", "lambdas", "captures"],
        port: Some("8112"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X POST -H "Content-Type: application/json" -d '{"tax":0.25,"items":[{"name":"tea","price":4,"qty":2},{"name":"cake","price":3,"qty":1}]}' http://127.0.0.1:8112/quote"#,
        text: include_str!("../samples/quote.holon.ts"),
    },
    Sample {
        name: "basket",
        title: "Basket (a collection read inside a function)",
        description: "Prices a basket from a price list kept in a collection: each line's function reads its item's price, so the read happens once per item, each answered on its own, recorded and replayed. An item with no price answers 404 instead of a made-up total.",
        shows: &["map", "fold", "collections", "reads inside a function"],
        port: Some("8113"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X PUT -d '{"price":4}' http://127.0.0.1:8113/prices/tea and again for cake at 3, then curl -X POST -d '{"items":[{"name":"tea","qty":2},{"name":"cake","qty":1}]}' http://127.0.0.1:8113/basket (an unpriced item: 404)"#,
        text: include_str!("../samples/basket.holon.ts"),
    },
    Sample {
        name: "tickets",
        title: "Tickets (counting to n)",
        description: "Hands out count numbered tickets for an event: range counts from 0 to count-1, and a function names each one, using the event from around it. range stops at 10000, so a map over it always ends; a count it can't count to answers 400.",
        shows: &["range", "map", "lambdas", "interpolation"],
        port: Some("8114"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X POST -d '{"event":"gig","count":3}' http://127.0.0.1:8114/tickets (a count of 2.5, -1 or 20000: 400)"#,
        text: include_str!("../samples/tickets.holon.ts"),
    },
    Sample {
        name: "cache-aside",
        title: "Cache-aside (a branch not taken)",
        description: "Hands out an id per name, fetched once (a fresh uuid from httpbin) and kept in a collection: a name it has an id for is answered from the collection, and the fetch, the ?? fallback it doesn't need, isn't run at all, so no request goes out. The run log and view mode show it as not run.",
        shows: &["coalesce", "lazy choices", "collections", "http"],
        port: Some("8115"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl http://127.0.0.1:8115/id/ada twice: the same id, and the second run sends no request (or set one: curl -X PUT -d '{"id":"x"}' http://127.0.0.1:8115/id/bob)"#,
        text: include_str!("../samples/cache-aside.holon.ts"),
    },
    Sample {
        name: "calc",
        title: "Calculator (a switch)",
        description: "Answers a + b, a - b, a * b or a / b by the op it is sent: a switch chooses one case, the others don't run, and a case chosen that has no value (a division by zero) answers 400 instead of some other case's value.",
        shows: &["switch", "lazy choices", "missing"],
        port: Some("8116"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -d '{"op":"add","a":1,"b":0}' http://127.0.0.1:8116/calc answers 1; with "op":"div" it answers 400"#,
        text: include_str!("../samples/calc.holon.ts"),
    },
    Sample {
        name: "loan",
        title: "Loan payments (JavaScript's numbers)",
        description: "The monthly payment on a loan, computed as JavaScript computes it: parseFloat and parseInt read the query, ** compounds the rate, Math.round rounds to the cent, toFixed writes the cents, and Math.floor and % split the term. A part that isn't a number has no value, where JavaScript gives NaN, so the route answers 400.",
        shows: &["number leaves", "Math", "parseFloat", "parseInt", "**", "%", "toFixed", "missing"],
        port: Some("8173"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl "http://127.0.0.1:8173/payment?amount=250000&rate=6.5&months=360" answers {"interest":"318861.20","payment":"1580.17","term":"30y 0m","total":"568861.20"}; with amount=lots it answers 400"#,
        text: include_str!("../samples/loan.holon.ts"),
    },
    Sample {
        name: "leaderboard",
        title: "Leaderboard (a list's methods)",
        description: "Post a round's scores and get the board JavaScript's list methods make of it: toSorted ranks by points (a tie keeps its order), slice and map's index give the top three their places, new Set names each player once, find the first to reach 100, some whether anyone scored nothing, and reduce the total. POST /board/:player filters one player's points, takes their best with reduce and no start, and finds where it ranks with findIndex. No list changes in place.",
        shows: &["list leaves", "toSorted", "filter", "find", "findIndex", "some", "new Set", "map with an index", "reduce without a start"],
        port: Some("8174"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -d '{"scores":[{"player":"ada","points":90},{"player":"bo","points":120},{"player":"cy","points":0},{"player":"ada","points":130}]}' http://127.0.0.1:8174/board answers {"any_zero":true,"first_to_100":"bo","players":["ada","bo","cy"],"top":["1. ada 130","2. bo 120","3. ada 90"],"total":340}; the same body to /board/ada answers {"best":130,"place":1,"player":"ada","points":[90,130],"rounds":2}, to /board/dee 404; a body without scores 400"#,
        text: include_str!("../samples/leaderboard.holon.ts"),
    },
    Sample {
        name: "invoice",
        title: "Invoice (records and dates)",
        description: "Post an order and get its invoice, made with JavaScript's record and date functions: a spread puts the order's defaults under what was posted, Object.groupBy groups its priced lines by category and Object.fromEntries gives each group's subtotal, in says whether it is a rush order, and new Date gives the dates: the day it was issued, the day it is due (setUTCDate past the month's end carries into the next) and the month's last day (Date.UTC with day 0), as ISO and UTC text. POST /invoice/lines writes the order as JSON text with only the listed keys, indented: JSON.stringify with a key list and an indent.",
        shows: &["record leaves", "spread", "Object.groupBy", "Object.fromEntries", "in", "new Date", "setUTCDate", "Date.UTC", "toISOString", "JSON.stringify with keys and an indent"],
        port: Some("8175"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -d '{"customer":"ada","date":"2026-11-07","lines":[{"sku":"b1","category":"books","price":12,"qty":2},{"sku":"p1","category":"pens","price":3,"qty":5},{"sku":"b2","category":"books","price":30,"qty":1}]}' http://127.0.0.1:8175/invoice answers {"categories":["books","pens"],"currency":"EUR","customer":"ada","due":"2026-12-07T00:00:00.000Z","due_text":"Mon, 07 Dec 2026 00:00:00 GMT","issued":"2026-11-07T00:00:00.000Z","month_end":"2026-11-30T00:00:00.000Z","rush":false,"subtotals":{"books":54,"pens":15},"total":69}; the same body to /invoice/lines answers the customer and each line's sku and qty, two spaces in; a body without lines 400"#,
        text: include_str!("../samples/invoice.holon.ts"),
    },
    Sample {
        name: "share-link",
        title: "Share link (a text's methods)",
        description: "An article's share link, built from the query with JavaScript's text methods: trim cuts the spaces around the title, toLowerCase and replaceAll make it a slug (an & reads \"and\"), split, filter and join drop the doubled spaces between its words, padStart writes the id in six digits, and encodeURIComponent escapes the tag for the link's query. + and a template literal join the parts, .length counts the title as JavaScript does, in UTF-16 code units, and startsWith says whether the link is secure. A query without a title or an id answers 400.",
        shows: &["text leaves", "trim", "replaceAll", "split", "padStart", "encodeURIComponent", "+ on texts", ".length", "startsWith"],
        port: Some("8177"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl "http://127.0.0.1:8177/link?title=%20Rust%20%26%20WebAssembly%20%20Tips%20&id=42&tag=c%2B%2B%20%26%20wasm" answers {"length":24,"secure":true,"slug":"rust-and-webassembly-tips","title":"Rust & WebAssembly  Tips","url":"https://example.com/a/000042/rust-and-webassembly-tips?tag=c%2B%2B%20%26%20wasm"}; with only id=42 it answers 400"#,
        text: include_str!("../samples/share-link.holon.ts"),
    },
    Sample {
        name: "heartbeat",
        title: "Heartbeat",
        description: "A clock flow: every 10 seconds it records the time, counts the beats in its state, and keeps the time of the first beat (a fallback that is itself a value: coalesce).",
        shows: &["clock", "every", "state across runs", "default", "coalesce", "add"],
        port: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "deploy it, wait, then look at its state on the deployed page (or GET /deployments/heartbeat/state)",
        text: include_str!("../samples/heartbeat.holon.ts"),
    },
    Sample {
        name: "rolling-log",
        title: "Rolling log (retention)",
        description: "A clock flow that keeps only its newest 5 runs, none older than a day: its `keep` option says how long a deployment's log keeps its runs (the deploy settings' keep_runs and keep_run_days), and keeping fewer than the host's default asks nobody. The states its cut runs left stay in the store until a sweep clears what nothing kept names: the server's own (holon.toml's [storage] sweep_every, ten minutes unless said), or an admin's holon store sweep.",
        shows: &["keep", "retention", "keep_runs", "keep_run_days", "clock", "state across runs", "holon store sweep"],
        port: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "deploy it, wait a few seconds: view mode's runs list holds the newest 5 (holon history rolling-log), its seqs climbing, while its state counts every beat; holon store sweep --dry-run counts the states no kept run names (an hour after they were put, holon store sweep removes them)",
        text: include_str!("../samples/rolling-log.holon.ts"),
    },
    Sample {
        name: "approval",
        title: "Order approval (durable run)",
        description: "An order waits for payment: the route that takes it starts the order's run and answers its pay link at once, and the run parks and finishes when the link is opened, hours or days later. A route answers and is done; what waits is the action it starts. Its progress goes out on the channel orders as it happens: the pack that doesn't wait for the payment as the run parks, the one that does when it finishes, never the first twice.",
        shows: &["start", "await", "resume routes", "durable runs", "channels", "progress from a waiting run"],
        port: Some("8105"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X POST -H "Content-Type: application/json" -d '{"item":"book"}' http://127.0.0.1:8105/orders (answers /paid/<token>), then open that path; the order is written to state "last_order". curl -N http://127.0.0.1:8105/orders/live shows each order's progress: "waiting for payment", then "paid""#,
        text: include_str!("../samples/approval.holon.ts"),
    },
    Sample {
        name: "fetch-json",
        title: "Fetch and reshape",
        description: "Call another API, parse its JSON and answer with the one field you need.",
        shows: &["http_request", "json_parse", "field", "host permissions"],
        port: Some("8106"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "curl http://127.0.0.1:8106/slideshow (calls https://httpbin.org/json, answers its title)",
        text: include_str!("../samples/fetch-json.holon.ts"),
    },
    Sample {
        name: "sign-payload",
        title: "Sign a payload",
        description: "Sign whatever is posted with a secret key (HMAC-SHA256) and answer the signature with the body in base64.",
        shows: &["hmac_sha256", "base64", "secrets", "record building", "holon verify with a secret"],
        port: Some("8107"),
        secrets: &[SampleSecret { name: "signing_key", what: "any secret of 6 characters or more to sign with", from: None }],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X POST -H "Content-Type: application/json" -d 'hello' http://127.0.0.1:8107/sign; then export its runs and holon verify checks each run given the signature its log entry kept (the key is never in the log)"#,
        text: include_str!("../samples/sign-payload.holon.ts"),
    },
    Sample {
        name: "browser-key",
        title: "Hand a browser key to a page",
        description: "Answer a page's configuration with a publishable key kept as a secret, so it can be rotated without a redeploy. Answering a caller with a secret is a permission of its own, which a person approves at deploy.",
        shows: &["secrets", "answering with a secret (SecretAnswer)", "approval", "record building"],
        port: Some("8163"),
        secrets: &[SampleSecret { name: "maps_browser_key", what: "the publishable key the page uses (6 characters or more)", from: None }],
        optional: &[],
        browsers: None,
        try_it: "deploy it (its approval lists answering with the secret), set the key under the gear, then curl http://127.0.0.1:8163/config; the run log shows the key as its placeholder",
        text: include_str!("../samples/browser-key.holon.ts"),
    },
    Sample {
        name: "echoed-key",
        title: "A key echoed back isn't passed on",
        description: "Send a key to one host, which echoes it back, and try to pass the echo to another: the host finds the key by its value and doesn't send it, since its approval shows it going only to the first.",
        shows: &["secrets", "where secrets go", "blocked_secret", "http_request", "verify"],
        port: Some("8164"),
        secrets: &[SampleSecret { name: "demo_key", what: "any made-up value (6 characters or more); it is sent to httpbin.dev", from: None }],
        optional: &[],
        browsers: None,
        try_it: "deploy it, set the key under the gear, then curl http://127.0.0.1:8164/echo: the second call answers blocked_secret",
        text: include_str!("../samples/echoed-key.holon.ts"),
    },
    Sample {
        name: "client-credentials",
        title: "A token exchange, and where the token goes",
        description: "Post a client secret to an OAuth server (Duende's public demo) for an access token, then call an API with the token (httpbin.dev stands in): the approval says the secret goes to the OAuth server and what is answered to it to the API, the canvas marks both requests, and the token is kept out of the log by its value.",
        shows: &["secrets", "where secrets go", "tokens", "OAuth", "http_request", "verify"],
        port: Some("8166"),
        secrets: &[SampleSecret { name: "client_secret", what: "secret, the demo server's published secret for its client m2m; it is sent to demo.duendesoftware.com", from: None }],
        optional: &[],
        browsers: None,
        try_it: "deploy it, set the secret under the gear, then curl http://127.0.0.1:8166/token answers {\"issued\":200,\"api\":200,\"authenticated\":true}; holon export client-credentials, then holon verify on it says where the token went",
        text: include_str!("../samples/client-credentials.holon.ts"),
    },
    Sample {
        name: "shared-key",
        title: "One key for every flow, capped",
        description: "Read a secret the server binds once for every flow that reads it, with where an admin says it may go: this flow sends it to httpbin.dev, so it deploys and reads the server's; one that sent it elsewhere would be refused at deploy, unless it had its own.",
        shows: &["secrets", "server-wide secrets", "where secrets go", "an admin's cap", "http_request"],
        port: Some("8167"),
        secrets: &[SampleSecret { name: "ops_api_key", what: "the server's: an admin binds it, holon secret bind ops_api_key --to httpbin.dev, any made-up value (6 characters or more)", from: None }],
        optional: &[],
        browsers: None,
        try_it: "first bind the key (an admin): printf 'ops-demo-key-123' | holon secret bind ops_api_key --to httpbin.dev; deploy it, then curl http://127.0.0.1:8167/whoami answers {\"status\":200,\"authenticated\":true}",
        text: include_str!("../samples/shared-key.holon.ts"),
    },
    Sample {
        name: "license-check",
        title: "A key in a query goes to the database",
        description: "Ask a database which plan a licence key unlocks, the key passed as the query's $1: the approval says the key is sent to the database \"vendor\", and the canvas marks the read. The row it answers is what the database said, not the key.",
        shows: &["secrets", "where secrets go", "databases", "SQL"],
        port: Some("8165"),
        secrets: &[SampleSecret { name: "license_key", what: "lic-demo-1234, the licence the vendor table holds; it is sent to the database \"vendor\"", from: None }],
        optional: &[],
        browsers: None,
        try_it: "first bind the database (an admin): holon database add vendor ~/holon-vendor.db --schema \"create table if not exists licenses (license_key text primary key, plan text not null, seats integer not null); insert or ignore into licenses values ('lic-demo-1234', 'team', 5)\"; deploy it, set the key under the gear, then curl http://127.0.0.1:8165/license answers {\"plan\":\"team\",\"seats\":5}",
        text: include_str!("../samples/license-check.holon.ts"),
    },
    Sample {
        name: "likes",
        title: "Likes counter (runs side by side)",
        description: "Each like reads a post's count from a collection, adds one and writes it back. Likes arriving at once run side by side; one whose count changed under it runs again, so none is lost.",
        shows: &["concurrent runs", "retries", "read-modify-write on a collection item", "collection_list", "as_text"],
        port: Some("8108"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "seq 50 | xargs -P 20 -I{} curl -s -X POST http://127.0.0.1:8108/posts/1/like, then GET /posts/1/likes (\"50 likes\") and GET /likes; the log marks runs that ran again",
        text: include_str!("../samples/likes.holon.ts"),
    },
    Sample {
        name: "counter",
        title: "Hit counter (writes run with their route)",
        description: "A route counts its hits though nothing in it reads the request: its writes are tied to the route's request. Another records the last visitor after the visit is put.",
        shows: &["a sink's trigger", "if (x !== undefined)", "state", "collection_put"],
        port: Some("8155"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "curl http://127.0.0.1:8155/hit twice (\"hit 2\"), curl -X POST http://127.0.0.1:8155/visits/ada, then GET /last (\"ada\")",
        text: include_str!("../samples/counter.holon.ts"),
    },
    Sample {
        name: "witness",
        title: "Run witness",
        description: "Takes the run heads a holon server sends it (holon serve --export-heads), with the witness key: keeps each batch with when it arrived and answers a receipt, so a run log changed afterwards no longer matches what was witnessed.",
        shows: &["run log hash chain", "--export-heads", "collections", "secrets", "secure_equals", "guard"],
        port: Some("8130"),
        secrets: &[SampleSecret { name: "witness_key", what: "the token servers send (their HOLON_EXPORT_HEADS_TOKEN)", from: None }],
        optional: &[],
        browsers: None,
        try_it: "start a server with HOLON_EXPORT_HEADS_TOKEN set to the witness key and --export-heads http://127.0.0.1:8130/heads, let any deployment run, then GET http://127.0.0.1:8130/batches",
        text: include_str!("../samples/witness.holon.ts"),
    },
    Sample {
        name: "ledger",
        title: "Ledger (every run checkable)",
        description: "Keeps a balance per account: each post adds its amount and answers the new balance with when it was posted. Export its runs and holon verify runs every posting again, offline, from what its log entry recorded (the balance it read, the clock), and checks it produced what the log says, and that each began from the balances the one before left.",
        shows: &["holon verify", "run export", "collections", "clock", "with_field"],
        port: Some("8131"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X POST -d '{"amount":5}' http://127.0.0.1:8131/accounts/ada a few times, save GET /deployments/<slug>/export (signed in) as runs.json, then holon verify runs.json"#,
        text: include_str!("../samples/ledger.holon.ts"),
    },
    Sample {
        name: "marketplace",
        title: "Marketplace (built with Holon)",
        description: "A marketplace for holons, itself a Holon flow: publishing a listing (a holon's name, description, ports, manifest and its units, by hash) takes the publish key; browsing and reading one are open to any web page; a bad version is yanked, never edited. The editor publishes saved flows to it and places what it lists, checking every unit against its hash.",
        shows: &["marketplace", "web pages from other sites (CORS)", "collections", "secrets", "secure_equals", "guard", "with_field"],
        port: Some("8120"),
        secrets: &[SampleSecret { name: "publish_key", what: "the key publishers send (Authorization: Bearer <key>) to publish or yank", from: None }],
        optional: &[],
        browsers: Some("*"),
        try_it: "set it as the editor's marketplace (flows page), publish a saved flow without routes from the flows page, and place it from the palette's marketplace; GET http://127.0.0.1:8120/listings lists them",
        text: include_str!("../samples/marketplace.holon.ts"),
    },
    Sample {
        name: "nearest",
        title: "Nearest word (a js node)",
        description: "Keep a dictionary of words and answer the closest one to a query by edit distance: the one dense computation is a js node, JavaScript run as a module with no imports under a fuel budget; the flow around it stores, routes and answers.",
        shows: &["js node", "module", "collections", "keys", "field"],
        port: Some("8117"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "curl -X POST --data-binary kitten http://127.0.0.1:8117/words, then sitting and mitten; curl http://127.0.0.1:8117/nearest/kittne answers { best: \"kitten\", distance: 2 }; a word nobody added yet answers 404",
        text: include_str!("../samples/nearest.holon.ts"),
    },
    Sample {
        name: "tally",
        title: "Tally a CSV (a js node parsing text)",
        description: "Post a CSV and get a glance at each column: the kind, min, max and mean of a number column, the distinct values and the top three of a text column. Parsing text into structure is a js node's work; the flow stores the last summary in its state and answers it.",
        shows: &["js node", "state", "multi-output", "records and lists out of JavaScript"],
        port: Some("8119"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "printf 'city,temp\nOslo,4\nRome,19\nOslo,6\n' | curl -s -X POST --data-binary @- http://127.0.0.1:8119/tally answers rows 3 and two columns (city: text, 2 distinct; temp: number, 4 to 19, mean 9.67); curl http://127.0.0.1:8119/tally/last repeats it with when",
        text: include_str!("../samples/tally.holon.ts"),
    },
    Sample {
        name: "idempotent",
        title: "Idempotent charges (one call per key)",
        description: "Charge once per Idempotency-Key: the same key with the same body answers the first charge again, the same key with a different body is refused (409), a key expires after a day, and the processor is called only when there is no prior charge. Two identical requests at once: the second waits for the first's claim on the key, reads what it wrote, and sends nothing (docs/L4s/key-claims-L4.md).",
        shows: &["idempotency key", "key claims", "a request tied to a claim", "sha256 fingerprint", "collection", "absence as refusal"],
        port: Some("8121"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "curl -X POST -H 'Idempotency-Key: k1' -H 'Content-Type: application/json' -d '{\"amount\":42}' http://127.0.0.1:8121/charges twice: the same id, the second replayed: true; send it with amount 43 for a 409; run two at once with a fresh key and `explain` shows the second claimed charges[k2], waited, and sent nothing",
        text: include_str!("../samples/idempotent.holon.ts"),
    },
    Sample {
        name: "guestbook",
        title: "Guestbook (a page from a flow)",
        description: "A web page served by the flow itself: a js node renders the guestbook as HTML from the entries in a collection, with the clock wired in to say how long ago each was signed; another parses the posted form; the flow stores a signature and redirects back to the page. The response is an envelope: status, headers (content-type: text/html) and body.",
        shows: &["js node", "HTML response (an envelope with headers)", "forms", "collections", "redirect (303)", "the clock wired into code"],
        port: Some("8122"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "open http://127.0.0.1:8122/ in a browser and sign it; or curl -s -d 'name=Ada&note=Hello' http://127.0.0.1:8122/sign -o /dev/null -w '%{http_code}' answers 303, and curl http://127.0.0.1:8122/ shows the entry",
        text: include_str!("../samples/guestbook.holon.ts"),
    },
    Sample {
        name: "inbox-report",
        title: "Inbox report (files in folders on the host)",
        description: "Reads a CSV from a folder on the host, counts its rows and totals its number columns in a js node, and writes a Markdown report to another folder; lists and serves the reports. The flow names its folders (inbox, reports), never a path: an admin binds each to a real folder, read-only or read-write, and deploying asks for each use. A write lands whole when the run finishes; the run log keeps what was read and a hash of what was written.",
        shows: &["named folders", "file reads and writes", "js node", "listing files"],
        port: Some("8123"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folders (an admin): holon folder add inbox ~/holon-inbox --read-only and holon folder add reports ~/holon-reports; put sales.csv (region,amount / north,10 / south,32) in the inbox; then curl -s -X POST -H 'content-type: application/json' -d '{\"name\":\"sales.csv\"}' http://127.0.0.1:8123/report answers rows 2, curl http://127.0.0.1:8123/reports/sales.csv.md shows the report (amount: 42), and curl http://127.0.0.1:8123/reports lists it",
        text: include_str!("../samples/inbox-report.holon.ts"),
    },
    Sample {
        name: "log-archive",
        title: "Log archive (big files, by handle and by range)",
        description: "Serves the last lines and the first bytes of a log of any size, and archives a log to another folder host to host. Only the lines or bytes asked for enter the flow, and the run log keeps with them the hashes that prove them part of the file, so holon verify checks the run without the file. Archiving reads a handle (name, size, hash), never the bytes: the copy streams them when the run finishes, and fails if the log changed since it was read.",
        shows: &["named folders", "file handles", "copy by handle", "lines and slices", "range proofs"],
        port: Some("8124"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folders (an admin): holon folder add logs ~/holon-logs --read-only and holon folder add archive ~/holon-archive; put any app.log in the logs folder; then curl http://127.0.0.1:8124/logs/app.log/tail answers its last 20 lines, curl http://127.0.0.1:8124/logs/app.log/head its first 256 bytes, curl -s -X POST -H 'content-type: application/json' -d '{\"name\":\"app.log\"}' http://127.0.0.1:8124/archive copies it into the archive under its hash, and curl http://127.0.0.1:8124/archive lists the archive",
        text: include_str!("../samples/log-archive.holon.ts"),
    },
    Sample {
        name: "journal",
        title: "Journal (a folder kept for a while, and capped)",
        description: "Appends each line posted to a page, a text file in a folder on the host, and serves the page. Every run reads the page whole, so the run log keeps what it read: past 4 KiB in the blob store, by content and in chunks, so a page that grows by a line adds a chunk, not a copy. The admin decides how long those texts are kept (keep: bytes for 7d, then only their hash) and how big the folder may get (a cap: a write past it doesn't land, and the run answers 500 saying so).",
        shows: &["named folders", "read then write", "kept for a while", "folder caps"],
        port: Some("8125"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folder (an admin): holon folder add journal ~/holon-journal --keep 7d --cap 10M; then curl -s -X POST -d 'first line' http://127.0.0.1:8125/journal/today and curl -s -X POST -d 'second line' http://127.0.0.1:8125/journal/today append to today.txt, curl http://127.0.0.1:8125/journal/today shows both lines, and curl http://127.0.0.1:8125/journal lists the pages; holon folder sweep removes now the texts no run kept within 7 days refers to",
        text: include_str!("../samples/journal.holon.ts"),
    },
    Sample {
        name: "live-board",
        title: "Live board (a channel: packs to whoever listens)",
        description: "A page that updates itself: posting a ticket stores it and sends a pack on the channel updates; every page open on the board is subscribed (GET /live answers with the channel's packs as server-sent events), and its EventSource adds the ticket as it arrives. A pack goes out once its run is over and its writes have landed, as the run is logged; a page that reconnects gets the packs it missed. Sending and letting callers subscribe are each a permission.",
        shows: &["channels", "server-sent events", "js node", "HTML response (an envelope with headers)", "collections"],
        port: Some("8126"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "open http://127.0.0.1:8126/ in two browsers and add a ticket in one: it shows in both; or curl -N http://127.0.0.1:8126/live in one terminal and curl -s -H 'content-type: application/json' -d '{\"title\":\"fix the door\"}' http://127.0.0.1:8126/tickets in another: the first prints id: … and data: {\"at\":…,\"title\":\"fix the door\"}",
        text: include_str!("../samples/live-board.holon.ts"),
    },
    Sample {
        name: "chat-proxy",
        title: "Chat proxy (a streamed reply)",
        description: "Asks Claude (the Messages API, streamed) what is posted, and forwards the answer to the caller as it arrives: the call's `stream: true` and the route's `respond(sent.body, { stream: true })` say so, and the page shows the words as they come. The flow still gets the whole answer once it ends, and stores the prompt with what was said (a js node reads the text out of the events). A call the API refuses (a bad key, a limit) forwards nothing: the reply is its answer, with its status, and the page says so. POST /room pipes the answer to the channel room instead (`room.pipe(anthropic.post(…))`): whoever watches GET /room/live gets its pieces as they arrive, then the whole of it once the run is over. The key is a secret; an answer has the run's 25 seconds (run_budget_ms under ⚙ for longer ones).",
        shows: &["streamed replies", "http_stream", "pipe", "channels", "server-sent events", "secrets", "js node", "collections"],
        port: Some("8127"),
        secrets: &[SampleSecret { name: "anthropic_api_key", what: "an Anthropic API key (sk-ant-…), from console.anthropic.com", from: None }],
        optional: &[],
        browsers: None,
        try_it: "open http://127.0.0.1:8127/ and ask something: the answer shows as it is written; or curl -N -H 'content-type: application/json' -d '{\"prompt\":\"a haiku about rivers\"}' http://127.0.0.1:8127/chat prints the events as they come. GET / lists what was asked before. For the room: curl -N http://127.0.0.1:8127/room/live in one terminal, then curl -s -H 'content-type: application/json' -d '{\"prompt\":\"a haiku about rivers\"}' http://127.0.0.1:8127/room in another: the first prints the pieces as they come, then {\"prompt\":…,\"said\":…}",
        text: include_str!("../samples/chat-proxy.holon.ts"),
    },
    Sample {
        name: "gemini-chat",
        title: "Gemini chat (a streamed reply, free tier)",
        description: "chat-proxy's streamed reply against Google's Gemini API (the Interactions API, streamed), whose free tier needs no credit: the answer goes to the caller as it arrives (`stream: true` on the call, `respond(sent.body, { stream: true })` on the route), the page shows the words as they come, and the flow stores the prompt with what was said once the answer ends (a js node reads the text out of the step.delta events). A call the API refuses (a bad key, the free tier's rate limit) forwards nothing: the reply is its answer, with its status, and the page says so. The key is a secret. On the free tier Google may use what is sent to improve its products.",
        shows: &["streamed replies", "http_stream", "server-sent events", "secrets", "js node", "collections"],
        port: Some("8128"),
        secrets: &[SampleSecret { name: "gemini_api_key", what: "a Gemini API key, free from aistudio.google.com (Get API key)", from: None }],
        optional: &[],
        browsers: None,
        try_it: "open http://127.0.0.1:8128/ and ask something: the answer shows as it is written; or curl -N -H 'content-type: application/json' -d '{\"prompt\":\"a haiku about rivers\"}' http://127.0.0.1:8128/chat prints the events as they come. GET / lists what was asked before.",
        text: include_str!("../samples/gemini-chat.holon.ts"),
    },
    Sample {
        name: "chess",
        title: "Chess against an agent (held to its interface)",
        description: "Agent governance: a Gemini agent plays Black against you, and can act only within the interface the app enforces. A js node holds the rules (legal moves, check, mate, the draws) and checks your move (an illegal one is 400 and changes nothing); the agent gets the whole game, the position, the material and the legal moves, and answers {move, comment, verdict, mood, say}, which the flow checks: a move not in the list, an answer that isn't the JSON asked for, or a call Gemini refuses plays the rules' own choice instead, and the page says so; a verdict or mood outside the words it was given is dropped. It reacts like a player across the board (your move brilliant, risky or insane; it struggles, wonders, gloats), and its chat replies stream: the call is piped to a channel, and the page writes the words as Gemini sends them, while the run keeps the whole reply. Its chat never reaches its moves (nothing typed there can steer the board). The page says how a game ends and how it ended (mate, resignation, the draws), and each game is played against a model picked from the newest few, from the app's list. Its limits give a run a minute and a Gemini call 50 seconds of it, for models that think: a slower answer is cut off, and its turn waits for you to ask it again or let the rules play (and while it waits you can't move for it). Every proposal and refusal is in the run log.",
        shows: &["agent governance", "structured output, checked", "js node", "fuel", "limit", "run_budget_ms", "request_timeout_ms", "secrets", "collections", "channels", "pipe", "HTML page", "separate calls"],
        port: Some("8168"),
        secrets: &[SampleSecret { name: "gemini_api_key", what: "a Gemini API key, free from aistudio.google.com (Get API key)", from: None }],
        optional: &[],
        browsers: None,
        try_it: "open http://127.0.0.1:8168/ and play White: drag a piece to its square, or click it and then the square; the page says your move is sent and counts the seconds while the Sphinx thinks; it answers each move, with its comment, what it makes of yours, its mood and whether it kept the contract beside the board, and now and then a line in the chat. Say something in the chat: it hints in riddles, its reply streaming into its bubble as Gemini writes it (curl -N http://127.0.0.1:8168/replies prints it as it streams). Mate it to win; Resign (pressed twice) loses. Click a move in the list (or use the arrow keys) to see the position after it; ⏭ goes back to the game. The chat is in bubbles, yours on the right, the Sphinx's on the left, its asides typed in. Pick the model for the next game beside New game (gemini-3.5-flash-lite is the quickest; gemini-4-argon needs a paid plan). Or curl -s -H 'content-type: application/json' -d '{\"move\":\"e2e4\"}' http://127.0.0.1:8168/move answers the board after its reply; {\"move\":\"e2e5\"} answers 400. If it doesn't answer in time, Try again asks it once more and Let the rules play plays the rules' choice. Without the key, or past the free tier's limit, the rules play for it and the page says so.",
        text: include_str!("../samples/chess.holon.ts"),
    },
    Sample {
        name: "shared-ledger",
        title: "Shared ledger (one file, several deployments)",
        description: "Appends each line posted to one file in a folder on the host, and serves it. Deployed twice, two flows write the same file at once and every line lands: a run commits only if the file is as it read it, and the check and the write hold the file's commit lock, which every deployment's runs share, so one never writes over another's (runs that touch other files don't wait for it). A run that finds the file changed runs again on the file as it now is.",
        shows: &["named folders", "read then write", "commit locks", "several deployments"],
        port: Some("8140"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folder (an admin): holon folder add ledger ~/holon-ledger; deploy it, then deploy a copy renamed shared-ledger-2 on port 8141; post to both at once, for i in $(seq 20); do curl -s -X POST -d \"a $i\" http://127.0.0.1:8140/lines & curl -s -X POST -d \"b $i\" http://127.0.0.1:8141/lines & done; wait, and curl http://127.0.0.1:8140/lines shows all 40 lines",
        text: include_str!("../samples/shared-ledger.holon.ts"),
    },
    Sample {
        name: "orders-api",
        title: "Orders API (a database on the host)",
        description: "Places, lists and refunds orders in a SQL database an admin binds on the host (database shop): a SQLite file, or a Postgres or MySQL database, the flow's text the same on each. Each query is written in the flow, its values passed as $1, $2, …, never into its text; its tables are the permission. Reads are answered as the run found them, and the writes land when the run finishes, in one transaction that first checks the rows it read are as it read them. A refund must change exactly one row (expect: 1): refunding an order twice changes none, so nothing lands and the run says so.",
        shows: &["databases", "SQL", "read then write", "expect"],
        port: Some("8150"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the database (an admin): holon database add shop ~/holon-shop.db --schema \"create table if not exists orders (id integer primary key, customer text not null, total numeric not null, refunded integer not null default 0); create table if not exists refunds (order_id integer not null, amount numeric not null, status integer)\"; deploy it, then curl -X POST -d '{\"customer\":\"ada\",\"total\":\"12.50\"}' http://127.0.0.1:8150/orders, curl http://127.0.0.1:8150/orders/ada, and curl -X POST http://127.0.0.1:8150/order/1/refund twice: the second changes no row. On Postgres instead: start the server with HOLON_SECRET_SHOP_URL=postgres://user:password@host/shop in its environment and bind holon database add shop env:HOLON_SECRET_SHOP_URL --schema \"create table if not exists orders (id integer generated by default as identity primary key, customer text not null, total numeric(10,2) not null, refunded integer not null default 0); create table if not exists refunds (order_id integer not null, amount numeric(10,2) not null, status integer)\" (MySQL: mysql://…, and id integer primary key auto_increment); totals then come back exact, as text",
        text: include_str!("../samples/orders-api.holon.ts"),
    },
    Sample {
        name: "refund-once",
        title: "Refund once (two requests, one payment)",
        description: "Refunds an order through a payment API (httpbin.org stands in) and records it, in the database orders-api writes. Two refunds of one order at the same moment call the payment API once: the first to call out holds the order's row, since it read it and may write it, until it commits; the second waits, finds the order refunded, and answers 409 having sent nothing. A row claim covers every deployment bound to the database, so orders-api's own refund waits too; on Postgres or MySQL it is held in the database itself, so it covers every server bound to it.",
        shows: &["databases", "claims", "read, call, then write", "several deployments"],
        port: Some("8151"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "bind shop as orders-api says (its schema makes both tables) and deploy orders-api and this; place an order (curl -X POST -d '{\"customer\":\"ada\",\"total\":5}' http://127.0.0.1:8150/orders), then refund it twice at once: curl -X POST http://127.0.0.1:8151/refund/1 & curl -X POST http://127.0.0.1:8151/refund/1; one answers 200, the other 409, and curl http://127.0.0.1:8151/refunds/1 lists one refund",
        text: include_str!("../samples/refund-once.holon.ts"),
    },
    Sample {
        name: "ticket-numbers",
        title: "Ticket numbers (a database held for whole runs)",
        description: "Hands out numbered tickets from one counter row: each request reads the last number, writes the next and records who has it. Its text is an ordinary database flow; what it shows is the admin's setting on the database: bound with --lock run, each run that may write it holds it from before it starts until it commits, so a burst of requests takes numbers one after the other, every run committing on its first try, none reading a number another is about to take and running again. Readers of the list hold it shared, side by side. The cost is said where it is set: the writers of the database go one at a time, across every deployment bound to it (and every server, on Postgres or MySQL); one that would wait past its claim_wait is turned away (503).",
        shows: &["databases", "run locks", "an admin's setting", "contention"],
        port: Some("8152"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the database (an admin): holon database add tickets ~/holon-tickets.db --lock run --schema \"create table if not exists counter (id integer primary key, n integer not null); insert or ignore into counter values (1, 0); create table if not exists issued (n integer primary key, holder text not null)\"; deploy, then take twenty at once: for i in $(seq 20); do curl -s -X POST -d '{\"name\":\"p'$i'\"}' http://127.0.0.1:8152/ticket & done; wait; curl http://127.0.0.1:8152/tickets lists 1 to 20, one each, and the deployment's log shows no run ran again",
        text: include_str!("../samples/ticket-numbers.holon.ts"),
    },
    Sample {
        name: "approved-payout",
        title: "Approved payout (a waiting run checks what it read)",
        description: "Asks for a payout from an account: the route answers with an approval link (202) and starts the payout's run, which waits for it, a day at most, holding nothing on the account meanwhile: a deposit goes ahead while it waits. Approved, the run reads the balance as it is now and holds the row from its payout until it commits, as any run does (a deposit meanwhile waits, and runs again on the new balance). And it checks what it read at its commit as any run does: if the balance was changed by a program outside Holon while the payout was in flight, it runs again from the approval, on the balance as it now is, the payout and the approval answered from its record, not sent or asked again. No deposit is written over, and nothing is paid twice.",
        shows: &["start", "durable runs", "databases", "checks at commit", "running again"],
        port: Some("8153"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the database (an admin): holon database add bank ~/holon-bank.db --schema \"create table if not exists accounts (id integer primary key, balance integer not null); insert or ignore into accounts values (1, 100); create table if not exists payouts (account integer not null, amount integer not null, status integer)\"; deploy, then curl -X POST -d '{\"amount\":30}' http://127.0.0.1:8153/payout/1 answers an approve link; curl -X POST -d '{\"amount\":50}' http://127.0.0.1:8153/deposit/1 (150); curl -X POST -d '{\"by\":\"ops\"}' http://127.0.0.1:8153<the link> pays it; curl http://127.0.0.1:8153/accounts/1 says 120, the deposit kept",
        text: include_str!("../samples/approved-payout.holon.ts"),
    },
    Sample {
        name: "atlas-notes",
        title: "Atlas notes (a MongoDB database)",
        description: "Keeps notes as documents in a MongoDB collection an admin binds on the host: a cluster on MongoDB Atlas, or a replica set of your own. Each call is a record literal written in the flow, its fields and operators ($in, $set, $inc) the flow's and each value from the request passed in as data, so a request can never add an operator; the collection is the permission. Finds are answered as the run found them; inserts, updates and deletes land when the run finishes, in one transaction that first checks what it found is still so. Marking a note done must match exactly one open note (expect: 1): twice, the second finds none, and nothing lands.",
        shows: &["databases", "MongoDB", "documents", "expect"],
        port: Some("8154"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "start the server with HOLON_SECRET_NOTES_URL=mongodb+srv://user:password@cluster0.example.mongodb.net/notes (Atlas; or mongodb://127.0.0.1:27017/notes?replicaSet=rs0) in its environment and bind it (an admin): holon database add notes env:HOLON_SECRET_NOTES_URL; deploy, then curl -X POST -d '{\"id\":\"n1\",\"text\":\"buy milk\",\"tags\":[\"home\"]}' http://127.0.0.1:8154/notes/ada, curl http://127.0.0.1:8154/notes/ada, curl http://127.0.0.1:8154/notes/ada/tagged/home, and curl -X POST http://127.0.0.1:8154/notes/ada/n1/done twice: the second finds no open note",
        text: include_str!("../samples/atlas-notes.holon.ts"),
    },
    Sample {
        name: "drop-box",
        title: "Drop box (a run per new file, sent on by handle)",
        description: "Runs once for each new file dropped into a folder on the host (on.file.created(drop, …)), when it stops changing: the handler is given the file's handle (name, size, hash). It sends the file to a server as the body of a PUT, by handle: the server streams the bytes from the folder, never through the flow, and the run log keeps only their hash. It writes a receipt for each file to another folder. Files already there when it is deployed aren't new; one it writes into a folder it watches doesn't start it again.",
        shows: &["on.file.created", "file handles", "a file as a request body", "named folders"],
        port: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folders (an admin): holon folder add drop ~/holon-drop --read-only and holon folder add receipts ~/holon-receipts; deploy, then copy any file into ~/holon-drop: within two seconds ~/holon-receipts holds <file>.json with its size, its hash and how many bytes httpbin.org received, and the deployment's log shows the run",
        text: include_str!("../samples/drop-box.holon.ts"),
    },
    Sample {
        name: "file-audit",
        title: "File audit (runs on several folders, removals and some names)",
        description: "Keeps an audit trail of two folders on the host, one entry per change: a CSV new or changed in the inbox (on.file.created(inbox, …, { matching: \"*.csv\" })), any file removed from it (on.file.deleted(inbox, …)), and any file new or changed in the outbox, each its own handler. Each is given the file's handle with its event (new, changed or deleted; a removed file as last seen, without a hash). Each entry is created, never replaced (audit.create): a write that would replace one fails the run.",
        shows: &["on.file.created", "several file handlers", "on.file.deleted", "matching", "create-only writes"],
        port: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folders (an admin): holon folder add inbox ~/holon-inbox --read-only, holon folder add outbox ~/holon-outbox --read-only and holon folder add audit ~/holon-audit; deploy, then copy a .csv into ~/holon-inbox, delete a file from it, or drop any file into ~/holon-outbox: within two seconds ~/holon-audit/<folder>/ holds an entry for each change; a .txt dropped into the inbox starts nothing",
        text: include_str!("../samples/file-audit.holon.ts"),
    },
    Sample {
        name: "file-checksums",
        title: "File checksums (each file's SHA-256, over its bytes)",
        description: "Runs once for each file new or changed in a folder on the host and reads its bytes (drop.bytes(name)): any file, an image, a PDF or an archive as well as a text. It keeps the file's size (b.length), its SHA-256 over the bytes (sha256(b), what sha256sum prints) and what its first four bytes say it is (b.slice(0, 4) as hex: a PNG, a PDF, a zip), under its name in a collection; routes read one back and list them all. The run log keeps a small file's bytes and a big one's hash, so holon verify checks each run from them.",
        shows: &["bytes", "a file's bytes", "sha256 over bytes", "slice on bytes", "bytes_to_text", "on.file.created", "named folders"],
        port: Some("8179"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folder (an admin): holon folder add drop ~/holon-drop --read-only; deploy, copy any file into ~/holon-drop, then curl http://127.0.0.1:8179/sums/<file> for its size, its SHA-256 (what sha256sum prints) and its kind (png, pdf, zip or other); GET /sums lists every file's",
        text: include_str!("../samples/file-checksums.holon.ts"),
    },
    Sample {
        name: "payout-totals",
        title: "Payout totals (exact decimals, over a CSV)",
        description: "Runs once for each CSV of payouts (id, currency, amount) new or changed in a folder on the host, reads it into records (csv_parse) and groups them by currency (Object.groupBy). A function of its own, summary, gives each group's count and total, over the groups' entries (Object.entries, e[0] the currency and e[1] its payouts): the amounts are summed as decimals written as text (decimal_sum), so the total is exact: 0.10 and 0.20 make 0.30, where numbers would make 0.30000000000000004; it is rounded to cents half to even, as a bank rounds (decimal_round). The count and total of each currency are kept under the file's name in a collection, and a route reads them back.",
        shows: &["exact decimals", "decimal_sum", "decimal_round", "csv_parse", "Object.groupBy", "Object.entries", "Object.fromEntries", "a function of its own", "on.file.created", "named folders"],
        port: Some("8180"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folder (an admin): holon folder add payouts ~/holon-payouts --read-only; deploy, save a CSV of id,currency,amount lines as ~/holon-payouts/may.csv (1,EUR,0.10 then 2,EUR,0.20 then 3,USD,1250.005), then curl http://127.0.0.1:8180/totals/may.csv answers {\"EUR\":{\"count\":2,\"total\":\"0.30\"},\"USD\":{\"count\":1,\"total\":\"1250.00\"}}",
        text: include_str!("../samples/payout-totals.holon.ts"),
    },
    Sample {
        name: "due-dates",
        title: "Due dates (business days at a fixed offset)",
        description: "Says when a payout is executed and credited: received before the 16:00 cut-off at the bank's offset (date_part with an offset reads its hours there), it is executed that business day, else the next (business_day_add over the closing days), and credited one business day later; the time received is written back at the offset (date_text). And when an invoice is due: some months after it was issued (date_add keeps the day of the month, or the month's last), moved to the next business day, with how many business days that is (business_days_between). The closing days are TARGET2's in 2026, a list the flow holds: a bank's calendar is data. The offset is fixed (Frankfurt is +01:00 in winter, +02:00 in summer), so a request may say which.",
        shows: &["business days", "holidays as data", "fixed offsets", "business_day_add", "business_days_between", "date_add", "date_part", "date_text", "now in a handler"],
        port: Some("8181"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "deploy, then curl -d '{\"received\":\"2026-04-02T16:30:00+02:00\",\"offset\":\"+02:00\"}' http://127.0.0.1:8181/schedule answers {\"credited\":\"2026-04-08\",\"executed\":\"2026-04-07\",\"received\":\"2026-04-02T16:30:00.000+02:00\"} (Good Friday and Easter Monday are closed); curl -d '{\"issued\":\"2026-01-31\",\"months\":1}' http://127.0.0.1:8181/due answers {\"business_days\":21,\"due\":\"2026-03-02\"}; leave received out for now",
        text: include_str!("../samples/due-dates.holon.ts"),
    },
    Sample {
        name: "ticker",
        title: "Ticker (a clock beside routes)",
        description: "A flow with a clock of its own beside its routes: every 2 seconds (every(\"2s\", …)) it counts a tick in its state and sends it on the channel ticks, with the time of the tick (now, in the handler). Its page lists each tick as it arrives; GET /count answers the count. An action, run by hand by its name, sets the count back (export const reset = action((input) => …), input what was sent); a person runs it, and an agent only once a person lets agents run it.",
        shows: &["every", "a clock beside routes", "now in a handler", "actions run by hand", "agents running an action", "channels", "state across runs"],
        port: Some("8156"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "open http://127.0.0.1:8156/: a new tick shows every 2 seconds; curl http://127.0.0.1:8156/count answers how many so far; run reset from the editor's server panel (or POST {\"to\": 0} to /trigger/ticker/reset on the holon server) to set it back, or from the deployed page; to let an agent run it (holon mcp's run tool), tick reset under ⚙ → agents may run, deploy and approve it",
        text: include_str!("../samples/ticker.holon.ts"),
    },
    Sample {
        name: "drop-board",
        title: "Drop board (files on a live page)",
        description: "A flow with file handlers beside its routes: each file that settles in a folder on the host (on.file.created(board, (f) => …), f the file's handle) counts in its state and goes out on the channel drops, and so does each file removed from it (on.file.deleted(board, …)). Its page lists each one as it happens.",
        shows: &["on.file.created", "file handlers beside routes", "on.file.deleted", "channels", "state across runs"],
        port: Some("8157"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "first bind the folder (an admin): holon folder add board ~/holon-board --read-only; deploy, open http://127.0.0.1:8157/, then copy any file into ~/holon-board: within two seconds the page shows it, with its size, and deleting it shows that too",
        text: include_str!("../samples/drop-board.holon.ts"),
    },
    Sample {
        name: "billing",
        title: "Billing (actions another flow runs)",
        description: "A flow whose work is two actions, run by name: charge records a charge in a collection and answers its total, refund deletes one. A person runs them from the editor; the sample shop runs them from its routes, a call from one flow to another's action. GET /charges/:id reads a charge back.",
        shows: &["actions", "actions another flow runs", "a collection", "an action that only writes"],
        port: Some("8158"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "deploy it first, then the sample shop; run charge from the editor's server panel with {\"id\": \"a1\", \"amount\": 5}, then curl http://127.0.0.1:8158/charges/a1; in the deployed shop, an order's run shows up here as an action run by the flow shop",
        text: include_str!("../samples/billing.holon.ts"),
    },
    Sample {
        name: "shop",
        title: "Shop (a flow calling another's actions)",
        description: "A flow that runs another deployed flow's actions: import { charge, refund } from \"flow:billing\" names them, and await charge(x) runs billing's action charge with x, answering {answer, run} or {error, message}. Deploying it asks to run the action charge and the action refund of the flow billing, which a person approves like any permission; an order is stored only when the charge answered, and answers 502 when billing can't be reached.",
        shows: &["flow: imports", "await charge(x)", "a call's answer and its errors", "a permission to run another flow's action"],
        port: Some("8159"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "deploy the sample billing first; then curl -X POST -H \"Content-Type: application/json\" -d '{\"id\":\"o1\",\"amount\":5}' http://127.0.0.1:8159/orders answers {\"id\":\"o1\",\"total\":6}; GET /orders/o1 reads the order, and billing's log has the charge run by the flow shop; stop billing and the same request answers 502; DELETE /orders/o1 runs billing's refund",
        text: include_str!("../samples/shop.holon.ts"),
    },
    Sample {
        name: "price-rollback",
        title: "Price rollback (an earlier version's code, or its prices, back)",
        description: "Every deploy is a version, kept for good, and every run keeps the state it left. Deploy this, set a price, then deploy a version with another fee: holon restore price-rollback --to N --code deploys an earlier one's code again, as a new version that names it, its prices as they are. Set a wrong price, and --to N --state puts back the prices version N left, in a new session, the wrong ones kept as the last session's; --to-run SEQ does the same for a run. Restored code is checked against today's grant, not the one it had. holon history shows it all among the runs: each deploy and restore, by whom. In the editor, view mode's history is the same timeline: pick any row in it, and restore its code, its state or both (a restore's row goes back to what it put back), or press ↺ on a row, each a dry run first, confirmed in a window that says what it changes, its diff showing the text, the grant and the prices beside what is there now. An agent sees the same through holon mcp: its history tool is the timeline, and its restore tool the dry run, never the restore; with ask, it saves the restore on the timeline for a person to approve there.",
        shows: &["versions", "holon restore", "state restore", "holon history", "the editor's history", "collections"],
        port: Some("8160"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X PUT -d '{"price":4}' http://127.0.0.1:8160/prices/tea, then curl http://127.0.0.1:8160/quote/tea answers a total of 4.5; change fee = 0.5 to fee = 5 and deploy again (a total of 9); holon restore price-rollback lists the versions, and holon restore price-rollback --to N --code (N: the one before) answers 4.5 again, tea still priced 4. Then curl -X PUT -d '{"price":40}' http://127.0.0.1:8160/prices/tea (a total of 40.5): holon restore price-rollback --to N --state --dry-run says what would change, and without --dry-run tea is 4 again; holon history price-rollback shows the runs, deploys and restores. Or in the editor: view, then history, pick the version before, and press code (then, after the wrong price, state; after another, ↺ on that restore's row puts its prices back)"#,
        text: include_str!("../samples/price-rollback.holon.ts"),
    },
    Sample {
        name: "stock-sheet",
        title: "Stock sheet (its state restored, its files not)",
        description: "Keeps stock counts in a collection and adds each change as a line to a file in a bound folder. Restoring its state puts the counts back, never the file: a file is an outside effect, as a call is. The restore's dry run names each file the runs since wrote, how many times and by which runs, and checks it on disk: one changed by anything but a run (an edit by hand) says so. Its diff shows the file's text then beside now, when the log has it (a run read it) and the file is small.",
        shows: &["restore", "dry run", "named folders", "file writes", "edits outside a run"],
        port: Some("8161"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"first bind the folder (an admin): holon folder add sheets ~/holon-sheets; then curl -X PUT -d '{"count":4}' http://127.0.0.1:8161/stock/tea and curl -X PUT -d '{"count":40}' http://127.0.0.1:8161/stock/tea; curl http://127.0.0.1:8161/sheet shows both lines. holon restore stock-sheet --to-run SEQ --state --dry-run (SEQ: the first PUT's run) puts tea back to 4 and names sheets/changes.txt, written once since, not put back; add a line to ~/holon-sheets/changes.txt by hand and the dry run says it was changed outside a run. In the editor: view, history, ↺ on the first PUT, then diff"#,
        text: include_str!("../samples/stock-sheet.holon.ts"),
    },
    Sample {
        name: "balance-trail",
        title: "Balance trail (which run changed a value)",
        description: "Moves money in and out of accounts, a balance per account in a collection and a count of moves in its state. A cell's history answers which run changed a value: the runs that changed one balance (or the count, or any account), newest first, each with the value before and after it, and a restore between two runs that moved it as a row of its own, so each value follows from the one before. holon history balance-trail --cell balances --item a prints it; in the editor, ⧗ beside a cell or an item in view mode's flow state lists it, each run a click away; an agent asks holon mcp's history tool with a cell.",
        shows: &["cell history", "state", "collections", "restore", "audit"],
        port: Some("8162"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X POST -d '{"amount":100}' http://127.0.0.1:8162/accounts/a/move, then the same with b and 5, and a with -30: curl http://127.0.0.1:8162/accounts/a answers 70. holon history balance-trail --cell balances --item a lists the two runs that moved a (nothing → 100, 100 → 70), not b's; --cell moves lists all three. Put the state the first run left back (holon restore balance-trail --to-run SEQ --state) and move a by 1: its history shows the restore between the runs (70 → 100) and the new run (100 → 101). In the editor: view, pick a run, Flow state, ⧗ beside balances or one of its items"#,
        text: include_str!("../samples/balance-trail.holon.ts"),
    },
    Sample {
        name: "status-board",
        title: "Status board (how each handler's runs are recorded)",
        description: "A log level per handler, as its author chooses: every run is recorded, and by default on disk before it answers (strict). A handler may say lazy (recorded all the same, on disk within a second, so it answers without waiting on the disk), here one that writes, or unrecorded (counted, not logged, failed or not; a person approves it).",
        shows: &["log levels", "strict", "lazy", "unrecorded", "collections"],
        port: Some("8170"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"deploy it (its approval asks to keep runs of GET /health and action report out of its log), then curl http://127.0.0.1:8170/health a few times ("ok", counted), curl -X POST -d '{"id":"a","text":"hi"}' http://127.0.0.1:8170/notes ("saved", strict), curl http://127.0.0.1:8170/notes (the notes, lazy) and curl -X POST -d '{"by":"sam"}' http://127.0.0.1:8170/notes/a/seen ("ok", lazy, though it writes): holon history status-board lists the notes runs and the health checks as one counted line, and view mode shows that line above the runs"#,
        text: include_str!("../samples/status-board.holon.ts"),
    },
    Sample {
        name: "stress-test",
        title: "Stress test (what each log level costs, under load)",
        description: "Three routes doing the same small work, recorded three ways: strict (on disk before it answers, a sync shared by the runs waiting at once), lazy (on disk within a second) and unrecorded (counted, not logged). holon load sends each as many requests as it answers and puts them side by side: requests a second, latency, and the runs the server recorded, synced and counted.",
        shows: &["log levels", "strict", "lazy", "unrecorded", "holon load"],
        port: Some("8171"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"deploy it (its approval asks to keep runs of GET /unrecorded out of its log), then holon load stress-test: each route for 10 s from 8 connections, a line a second, then the three side by side (req/s, p50, p90, p99, the slowest, errors, and the runs recorded, the log's syncs, the runs counted). holon load stress-test --for 5 --connections 32 --route "GET /strict" loads one harder; curl http://127.0.0.1:8171/strict answers "strict ok""#,
        text: include_str!("../samples/stress-test.holon.ts"),
    },
    Sample {
        name: "dexpaprika",
        title: "An n8n node, translated (DexPaprika)",
        description: "The n8n community node n8n-nodes-dexpaprika (MIT), translated by hand into Holon text, none of its JavaScript run: each operation an async function awaiting .get on an imported host (the permission), the optional credential an optional secret import the approval says goes to those hosts only, parameters as arguments, the node's path check in Holon, and its query string escaped by one js() function. docs/L4s/n8n-import-L4.md has the mapping.",
        shows: &["n8n import", "http_request", "holons", "optional secrets", "where secrets go", "js node", "host permissions"],
        port: Some("8172"),
        secrets: &[SampleSecret { name: "dexpaprika_key", what: "optional: a DexPaprika key (console.dexpaprika.com), sent as the whole Authorization header for a higher rate limit and the pro operations; without it the free tier answers", from: None }],
        // Without a key it calls the free tier: revoking the secret doesn't pause it.
        optional: &[r#"SecretRead("dexpaprika_key")"#],
        browsers: None,
        try_it: r#"curl http://127.0.0.1:8172/networks lists the networks; curl "http://127.0.0.1:8172/search?query=weth" searches every network; curl "http://127.0.0.1:8172/networks/ethereum/pools?limit=3" the top three pools; an id with a / ? # or % answers 400 and nothing is sent"#,
        text: include_str!("../samples/dexpaprika.holon.ts"),
    },
    Sample {
        name: "intake",
        title: "Intake (try, catch, throw and finally)",
        description: "Takes orders as JSON and keeps one only when every step of a try block succeeded: a function that throws new Error(…) for an order with no lines or a total not over 0, a body that isn't JSON, and a line without a price each answer the catch's 400, with nothing kept, and a throw's message goes to the run's log. finally counts every attempt, kept or not. A throw nothing catches is nothing, which a ?? answers.",
        shows: &["try and catch", "throw", "finally", "fail"],
        port: Some("8178"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X POST -d '{"id":"o1","lines":[{"price":2},{"price":3}]}' http://127.0.0.1:8178/orders answers 201 and keeps it; -d '{"id":"o2","lines":[]}' answers 400, "an order has lines" in the run's log; -d 'not json' answers 400; curl http://127.0.0.1:8178/stats counts all three; a /check of an order without lines answers 422"#,
        text: include_str!("../samples/intake.holon.ts"),
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
            // What starts it: requests on its port, or, without one, its clock or new files (an
            // `every` or `on.file` handler; beside routes, the port is what the entry says).
            let handled = s.text.lines().any(|l| l.starts_with("every(") || l.starts_with("on.file."));
            assert!(s.port.is_some() || handled, "{}: a port, a clock or new files", s.name);
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
            assert_eq!(server.browsers.as_deref(), s.browsers, "{}: browsers", s.name);
            let optional: Vec<String> = server.requirements.and_then(|r| r.optional).unwrap_or_default();
            assert_eq!(optional, s.optional, "{}: optional", s.name);
            // The secrets it imports (`import key from "secret:key"`) are the ones the entry says
            // how to set.
            let imported: Vec<&str> = s.text.lines().filter_map(|l| l.strip_prefix("import ")?.split_once(" from \"secret:")?.1.split('"').next()).collect();
            let said: Vec<&str> = s.secrets.iter().map(|x| x.name).collect();
            assert_eq!(imported.iter().copied().collect::<HashSet<_>>(), said.iter().copied().collect::<HashSet<_>>(), "{}: its secrets", s.name);
        }
        // rolling-log's `keep` is the limits the copy opened asks for.
        let limits = build(sample("rolling-log").unwrap()).unwrap().server.and_then(|v| v.requirements).and_then(|r| r.extra.get("limits").cloned());
        assert_eq!(limits, Some(serde_json::json!({ "keep_runs": 5, "keep_run_days": 1 })));
    }

    /// Each file is in the canonical form: `holon fmt` would leave it as it is, its comments and
    /// all. Each says, in them, what it does and how to try it. `HOLON_BLESS=1` writes each as fmt
    /// does, for a change to fmt's layout. fmt runs on 1 MiB of stack, what a Windows program's
    /// main thread has, where `holon fmt` runs it (a test's thread has 2): a sample fmt can't
    /// lay out there overflows it, and this test's process with it.
    #[test]
    fn every_sample_is_as_fmt_writes_it() {
        let on_a_main_thread = std::thread::Builder::new().name("fmt on a main thread's stack".into()).stack_size(1 << 20);
        on_a_main_thread
            .spawn(|| {
                for s in samples() {
                    let file = format!("{}.holon.ts", s.name);
                    let printed = crate::commands::fmt(&file, s.text).unwrap_or_else(|r| panic!("{}: {r:?}", s.name));
                    if std::env::var_os("HOLON_BLESS").is_some() && printed != s.text {
                        std::fs::write(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("samples").join(&file), &printed).unwrap();
                        continue;
                    }
                    assert_eq!(printed, s.text, "{}: samples/{file} isn't as fmt writes it", s.name);
                    assert!(s.text.starts_with("// "), "{}: samples/{file} opens with a comment saying what it is", s.name);
                }
            })
            .unwrap()
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    }

    #[test]
    fn a_mistake_in_a_sample_names_it_and_the_line() {
        let broken = Sample { text: "import { flow, route } from \"holon\"\n\nflow(\"hello-api\", { port: 8101 })\n\nroute.get(\"/hello/:name\", (req) => `Hello, ${req.params.nmae.x}${nope}`)\n", ..sample("hello-api").unwrap().clone() };
        assert_eq!(build(&broken).unwrap_err(), r#"sample "hello-api": 5:66: unknown name "nope""#);
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

    /// The server validates every unit of a closure before it runs one (crates/holon/src/units.rs
    /// `load_closure`): a sample with a unit `validate` refuses would never deploy.
    #[test]
    fn every_samples_units_pass_validation() {
        let mut checked = 0;
        for s in samples() {
            let lowered = lang::compile(s.text).unwrap_or_else(|e| panic!("{}: {e}", s.name));
            let mut store = MapResolver::new();
            crate::text::restore(&lowered.doc, &mut store, &Names::new());
            let port = lowered.doc.server.as_ref().map(|x| x.port.clone()).unwrap_or_default();
            crate::flow::flow_unit(&lowered.doc.nodes, &lowered.doc.edges, &mut store, &port).unwrap_or_else(|e| panic!("{}: {e}", s.name));
            for unit in store.units() {
                if let Err(errors) = holon_core::validate(unit, &store) {
                    panic!("{}: {:?} refused: {errors:?}", s.name, holon_core::hash_unit(unit).map(holon_core::hex));
                }
                checked += 1;
            }
        }
        assert!(checked > 100, "{checked}");
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
