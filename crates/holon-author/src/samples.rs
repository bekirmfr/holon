//! Sample flows: one per capability, opened from the editor's flows page as an unsaved copy (and
//! written by `holon new --sample`). Each is built from the live catalog and the template
//! interfaces through the flow builder, never from hand-copied node shapes, so a sample can't
//! drift from core: the editor's samples.test.ts builds every one and derives it through the real
//! core. Every new feature ships with a sample that shows it (CLAUDE.md).

use serde::Serialize;

use crate::builder::{FlowBuilder, FlowDocument, Holon, Requirements, Settings};
use crate::canvas::Position;

type R<T = ()> = Result<T, String>;

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

/// A sample: what the flows page shows of it, and how it's built.
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
    #[serde(skip)]
    build: fn(&mut B) -> R,
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
/// closure) in it, as a saved flow's are.
pub fn build(sample: &Sample) -> R<FlowDocument> {
    let mut fb = FlowBuilder::new();
    (sample.build)(&mut B(&mut fb)).map_err(|e| format!("sample \"{}\": {e}", sample.name))?;
    Ok(fb.finish(Settings {
        port: sample.port.unwrap_or_default().into(),
        interval: sample.interval.unwrap_or_default().into(),
        requirements: (!sample.optional.is_empty()).then(|| Requirements { optional: Some(sample.optional.iter().map(|s| s.to_string()).collect()), ..Default::default() }),
        browsers: sample.browsers.map(Into::into),
        ..Default::default()
    }))
}

/// The builder as the samples use it: every node at a position, params as `(name, value)`.
struct B<'a>(&'a mut FlowBuilder);

impl B<'_> {
    /// A leaf from the catalog or a template, with params set by name, at a canvas position.
    fn node(&mut self, name: &str, params: &[(&str, &str)], [x, y]: [i32; 2]) -> R<String> {
        let set: Vec<(String, String)> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        self.0.node(name, &set, Some(Position { x: x.into(), y: y.into() }))
    }

    fn wire(&mut self, from: &str, output: &str, to: &str, input: &str) -> R {
        self.0.wire(from, output, to, input)
    }

    /// Build a flow of its own and group it into one composite, as the editor's "group" does: its
    /// free inputs and outputs become the composite's ports, named in boundary order.
    fn holon(&mut self, name: &str, build: impl FnOnce(&mut Self) -> R, inputs: &[&str], outputs: &[&str]) -> R<Holon> {
        let strings = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        self.0.begin_holon(name);
        build(self)?;
        self.0.end_holon(name, &strings(inputs), &strings(outputs))
    }

    /// Place a holon as a node, wired by its port names like any other.
    fn place(&mut self, holon: &Holon, [x, y]: [i32; 2]) -> String {
        self.0.place(holon, Some(Position { x: x.into(), y: y.into() }))
    }
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
        build: hello_api,
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
        build: notes_api,
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
        build: stripe_to_slack,
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
        try_it: r#"curl -H "Authorization: Bearer <api_token>" http://127.0.0.1:8109/me answers a receipt; without it, or after the token is rotated, 401; revoke receipt_key and it answers "unsigned""#,
        build: guarded_api,
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
        build: slugs,
    },
    Sample {
        name: "heartbeat",
        title: "Heartbeat",
        description: "A clock flow: every 10 seconds it records the time and counts the beats in its state.",
        shows: &["clock", "interval", "state across runs", "default", "add"],
        port: None,
        interval: Some("10"),
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "deploy it, wait, then look at its state on the deployed page (or GET /deployments/heartbeat/state)",
        build: heartbeat,
    },
    Sample {
        name: "approval",
        title: "Order approval (durable run)",
        description: "An order waits for payment: the run hands out a pay link, parks, and finishes when the link is opened, hours or days later.",
        shows: &["await", "resume_token", "resume routes", "durable runs"],
        port: Some("8105"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: r#"curl -X POST -H "Content-Type: application/json" -d '{"item":"book"}' http://127.0.0.1:8105/orders (answers /paid/<token>), then open that path; the order is written to state "last_order""#,
        build: approval,
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
        build: fetch_json,
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
        build: sign_payload,
    },
    Sample {
        name: "likes",
        title: "Likes counter (runs side by side)",
        description: "Each like reads a post's count from a collection, adds one and writes it back. Likes arriving at once run side by side; one whose count changed under it runs again, so none is lost.",
        shows: &["concurrent runs", "retries", "read-modify-write on a collection item", "collection_list"],
        port: Some("8108"),
        interval: None,
        secrets: &[],
        optional: &[],
        browsers: None,
        try_it: "seq 50 | xargs -P 20 -I{} curl -s -X POST http://127.0.0.1:8108/posts/1/like, then GET /posts/1/likes (50) and GET /likes; the log marks runs that ran again",
        build: likes,
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
        build: marketplace,
    },
];

const OK: &str = "200";

fn respond(b: &mut B, ok: &str, missing: &str, at: [i32; 2]) -> R<String> {
    b.node("respond", &[("status", ok), ("missing", missing)], at)
}

fn text(b: &mut B, value: &str, at: [i32; 2]) -> R<String> {
    b.node("const", &[("value", value), ("type", "Text")], at)
}

/// A `field` node reading `name` from `record`'s output `output`.
fn field_of(b: &mut B, name: &str, record: &str, output: &str, at: [i32; 2]) -> R<String> {
    let f = b.node("field", &[("name", name)], at)?;
    b.wire(record, output, &f, "record")?;
    Ok(f)
}

fn hello_api(b: &mut B) -> R {
    let req = b.node("request", &[("method", "GET"), ("path", "/hello/:name")], [40, 40])?;
    let who = b.node("field", &[("name", "name")], [380, 60])?;
    let hello = text(b, "Hello, ", [380, 180])?;
    let greeting = b.node("concat", &[], [640, 100])?;
    let res = respond(b, OK, "404", [880, 60])?;
    b.wire(&req, "params", &who, "record")?;
    b.wire(&hello, "out", &greeting, "a")?;
    b.wire(&who, "out", &greeting, "b")?;
    b.wire(&req, "request", &res, "request")?;
    b.wire(&greeting, "out", &res, "body")
}

fn notes_api(b: &mut B) -> R {
    // GET /notes: a page of notes in id order, from after ?after= when it's given.
    let list = b.node("request", &[("method", "GET"), ("path", "/notes")], [40, 40])?;
    let after = b.node("field", &[("name", "after")], [480, 60])?;
    let from = b.node("default", &[("value", ""), ("type", "Text")], [720, 60])?;
    let page = b.node("collection_list", &[("collection", "notes"), ("limit", "50")], [960, 60])?;
    let list_res = respond(b, OK, "404", [1220, 40])?;
    b.wire(&list, "query", &after, "record")?;
    b.wire(&after, "out", &from, "value")?;
    b.wire(&from, "out", &page, "after")?;
    b.wire(&list, "request", &list_res, "request")?;
    b.wire(&page, "out", &list_res, "body")?;

    // PUT /notes/:id: keep the body under the id, answer with it.
    let put = b.node("request", &[("method", "PUT"), ("path", "/notes/:id")], [40, 340])?;
    let keep = b.node("collection_put", &[("collection", "notes")], [960, 380])?;
    let put_res = respond(b, OK, "400", [1220, 320])?;
    let id = field_of(b, "id", &put, "params", [480, 360])?;
    b.wire(&id, "out", &keep, "key")?;
    b.wire(&put, "json", &keep, "value")?;
    b.wire(&put, "request", &put_res, "request")?;
    b.wire(&put, "json", &put_res, "body")?;

    // GET /notes/:id: one note, or 404.
    let get = b.node("request", &[("method", "GET"), ("path", "/notes/:id")], [40, 640])?;
    let found = b.node("collection_get", &[("collection", "notes")], [960, 680])?;
    let get_res = respond(b, OK, "404", [1220, 640])?;
    let id = field_of(b, "id", &get, "params", [480, 660])?;
    b.wire(&id, "out", &found, "key")?;
    b.wire(&get, "request", &get_res, "request")?;
    b.wire(&found, "out", &get_res, "body")?;

    // DELETE /notes/:id: drop it, answer 204.
    let del = b.node("request", &[("method", "DELETE"), ("path", "/notes/:id")], [40, 940])?;
    let drop = b.node("collection_delete", &[("collection", "notes")], [960, 980])?;
    let del_res = b.node("respond", &[("status", "204")], [1220, 940])?;
    let id = field_of(b, "id", &del, "params", [480, 960])?;
    b.wire(&id, "out", &drop, "key")?;
    b.wire(&del, "request", &del_res, "request")
}

fn stripe_to_slack(b: &mut B) -> R {
    let req = b.node("request", &[("method", "POST"), ("path", "/stripe")], [40, 40])?;
    let signing = b.node("secret", &[("name", "stripe_webhook_secret")], [40, 300])?;
    let clock = b.node("clock", &[], [40, 400])?;
    let verify = b.node("stripe_verify", &[("tolerance", "300")], [380, 240])?;
    let kind = b.node("field", &[("name", "type")], [680, 160])?;
    let prefix = text(b, "Stripe: ", [680, 40])?;
    let message_text = b.node("concat", &[], [940, 80])?;
    let empty = b.node("record", &[], [940, 220])?;
    let message = b.node("with_field", &[("name", "text")], [1180, 140])?;
    let body = b.node("json_stringify", &[], [1420, 140])?;
    let no_headers = b.node("record", &[], [940, 340])?;
    let json_type = text(b, "application/json", [940, 430])?;
    let headers = b.node("with_field", &[("name", "content-type")], [1180, 360])?;
    let path = b.node("secret", &[("name", "slack_webhook_path")], [1180, 520])?;
    let post = b.node("http_request", &[("method", "POST"), ("host", "hooks.slack.com")], [1660, 300])?;
    let status = b.node("field", &[("name", "status")], [1920, 300])?;
    let res = respond(b, OK, "400", [2160, 120])?;
    b.wire(&req, "request", &verify, "request")?;
    b.wire(&signing, "out", &verify, "secret")?;
    b.wire(&clock, "out", &verify, "now")?;
    b.wire(&verify, "event", &kind, "record")?;
    b.wire(&prefix, "out", &message_text, "a")?;
    b.wire(&kind, "out", &message_text, "b")?;
    b.wire(&empty, "out", &message, "record")?;
    b.wire(&message_text, "out", &message, "value")?;
    b.wire(&message, "out", &body, "value")?;
    b.wire(&no_headers, "out", &headers, "record")?;
    b.wire(&json_type, "out", &headers, "value")?;
    b.wire(&path, "out", &post, "path")?;
    b.wire(&headers, "out", &post, "headers")?;
    b.wire(&body, "out", &post, "body")?;
    b.wire(&post, "out", &status, "record")?;
    b.wire(&req, "request", &res, "request")?;
    b.wire(&status, "out", &res, "body")
}

fn guarded_api(b: &mut B) -> R {
    let req = b.node("request", &[("method", "GET"), ("path", "/me")], [40, 40])?;
    let auth = b.node("field", &[("name", "authorization")], [320, 40])?;
    let bearer = text(b, "Bearer ", [40, 300])?;
    let token = b.node("secret", &[("name", "api_token")], [40, 400])?;
    let expected = b.node("concat", &[], [320, 320])?;
    let matches = b.node("secure_equals", &[], [580, 160])?;
    let path = b.node("field", &[("name", "path")], [320, 480])?;
    let key = b.node("secret", &[("name", "receipt_key")], [320, 600])?;
    let receipt = b.node("hmac_sha256", &[("encoding", "hex")], [580, 520])?;
    let or_unsigned = b.node("default", &[("value", "unsigned"), ("type", "Text")], [840, 540])?;
    let empty = b.node("record", &[], [580, 360])?;
    let body = b.node("with_field", &[("name", "receipt")], [840, 400])?;
    let allowed = b.node("guard", &[], [1080, 220])?;
    let res = respond(b, OK, "401", [1320, 40])?;
    b.wire(&req, "headers", &auth, "record")?;
    b.wire(&bearer, "out", &expected, "a")?;
    b.wire(&token, "out", &expected, "b")?;
    b.wire(&auth, "out", &matches, "a")?;
    b.wire(&expected, "out", &matches, "b")?;
    b.wire(&req, "request", &path, "record")?;
    b.wire(&key, "out", &receipt, "key")?;
    b.wire(&path, "out", &receipt, "message")?;
    b.wire(&empty, "out", &body, "record")?;
    b.wire(&receipt, "out", &or_unsigned, "value")?;
    b.wire(&or_unsigned, "out", &body, "value")?;
    b.wire(&matches, "out", &allowed, "cond")?;
    b.wire(&body, "out", &allowed, "value")?;
    b.wire(&req, "request", &res, "request")?;
    b.wire(&allowed, "out", &res, "body")
}

fn slugs(b: &mut B) -> R {
    // A holon that only computes: one Text in, its slug out.
    let slugify = b.holon(
        "slugify",
        |inner| {
            let lower = inner.node("lowercase", &[], [0, 0])?;
            let space = text(inner, " ", [0, 120])?;
            let words = inner.node("split", &[], [240, 40])?;
            let dash = text(inner, "-", [240, 160])?;
            let slug = inner.node("join", &[], [480, 60])?;
            inner.wire(&lower, "out", &words, "text")?;
            inner.wire(&space, "out", &words, "sep")?;
            inner.wire(&words, "out", &slug, "list")?;
            inner.wire(&dash, "out", &slug, "sep")
        },
        &["text"],
        &["slug"],
    )?;
    // A holon that reads and writes outside: the post goes into the "posts" collection under its
    // slug, with its author and the time; the last slug into state; and a receipt, signed when
    // the receipt key is granted, "unsigned" when it isn't.
    let publish = b.holon(
        "publish",
        |inner| {
            let slug = inner.node("default", &[("value", ""), ("type", "Text")], [0, 0])?;
            let none = inner.node("record", &[], [0, 120])?;
            let by_author = inner.node("with_field", &[("name", "author")], [240, 120])?;
            let clock = inner.node("clock", &[], [240, 260])?;
            let post = inner.node("with_field", &[("name", "at")], [480, 160])?;
            let put = inner.node("collection_put", &[("collection", "posts")], [720, 40])?;
            let last = inner.node("store_write", &[("key", "last_post")], [720, 180])?;
            let key = inner.node("secret", &[("name", "receipt_key")], [240, 380])?;
            let sig = inner.node("hmac_sha256", &[("encoding", "hex")], [480, 360])?;
            let receipt = inner.node("default", &[("value", "unsigned"), ("type", "Text")], [720, 360])?;
            inner.wire(&none, "out", &by_author, "record")?;
            inner.wire(&by_author, "out", &post, "record")?;
            inner.wire(&clock, "out", &post, "value")?;
            inner.wire(&slug, "out", &put, "key")?;
            inner.wire(&post, "out", &put, "value")?;
            inner.wire(&slug, "out", &last, "value")?;
            inner.wire(&key, "out", &sig, "key")?;
            inner.wire(&slug, "out", &sig, "message")?;
            inner.wire(&sig, "out", &receipt, "value")
        },
        &["slug", "author"],
        &["put", "saved", "receipt"],
    )?;
    // POST /slugs: slug the title and the author, publish the post, answer with both and the receipt.
    let req = b.node("request", &[("method", "POST"), ("path", "/slugs")], [40, 40])?;
    let title = b.node("field", &[("name", "title")], [380, 40])?;
    let author = b.node("field", &[("name", "author")], [380, 240])?;
    let title_slug = b.place(&slugify, [640, 40]);
    let author_slug = b.place(&slugify, [640, 240]);
    let published = b.place(&publish, [900, 380]);
    let empty = b.node("record", &[], [640, 560])?;
    let with_title = b.node("with_field", &[("name", "title")], [900, 120])?;
    let with_author = b.node("with_field", &[("name", "author")], [1140, 200])?;
    let with_receipt = b.node("with_field", &[("name", "receipt")], [1380, 300])?;
    let res = respond(b, OK, "400", [1620, 40])?;
    b.wire(&req, "json", &title, "record")?;
    b.wire(&req, "json", &author, "record")?;
    b.wire(&title, "out", &title_slug, "text")?;
    b.wire(&author, "out", &author_slug, "text")?;
    b.wire(&title_slug, "slug", &published, "slug")?;
    b.wire(&author_slug, "slug", &published, "author")?;
    b.wire(&empty, "out", &with_title, "record")?;
    b.wire(&title_slug, "slug", &with_title, "value")?;
    b.wire(&with_title, "out", &with_author, "record")?;
    b.wire(&author_slug, "slug", &with_author, "value")?;
    b.wire(&with_author, "out", &with_receipt, "record")?;
    b.wire(&published, "receipt", &with_receipt, "value")?;
    b.wire(&req, "request", &res, "request")?;
    b.wire(&with_receipt, "out", &res, "body")?;
    // GET /slugs: what was published, by slug.
    let list = b.node("request", &[("method", "GET"), ("path", "/slugs")], [40, 720])?;
    let posts = b.node("collection_list", &[("collection", "posts"), ("limit", "100")], [640, 740])?;
    let list_res = respond(b, OK, "404", [1620, 720])?;
    b.wire(&list, "request", &list_res, "request")?;
    b.wire(&posts, "out", &list_res, "body")
}

fn heartbeat(b: &mut B) -> R {
    let clock = b.node("clock", &[], [40, 40])?;
    let last = b.node("store_write", &[("key", "last_beat")], [300, 40])?;
    let read = b.node("store_read", &[("key", "beats")], [40, 200])?;
    let count = b.node("default", &[("value", "0"), ("type", "Number")], [260, 200])?;
    let one = b.node("const", &[("value", "1"), ("type", "Number")], [260, 320])?;
    let more = b.node("add", &[], [520, 240])?;
    let save = b.node("store_write", &[("key", "beats")], [760, 240])?;
    b.wire(&clock, "out", &last, "value")?;
    b.wire(&read, "out", &count, "value")?;
    b.wire(&count, "out", &more, "a")?;
    b.wire(&one, "out", &more, "b")?;
    b.wire(&more, "out", &save, "value")
}

fn approval(b: &mut B) -> R {
    // POST /orders: answer with the pay link, then wait at "paid".
    let order = b.node("request", &[("method", "POST"), ("path", "/orders")], [40, 40])?;
    let token = b.node("resume_token", &[("await", "paid")], [380, 40])?;
    let base = text(b, "/paid/", [380, 140])?;
    let link = b.node("concat", &[], [640, 60])?;
    let res = respond(b, OK, "404", [880, 40])?;
    let paid = b.node("await", &[("name", "paid"), ("expires", "86400")], [380, 300])?;
    let via = b.node("field", &[("name", "method")], [640, 300])?;
    let done = b.node("with_field", &[("name", "paid_via")], [880, 300])?;
    let save = b.node("store_write", &[("key", "last_order")], [1120, 300])?;
    b.wire(&base, "out", &link, "a")?;
    b.wire(&token, "out", &link, "b")?;
    b.wire(&order, "request", &res, "request")?;
    b.wire(&link, "out", &res, "body")?;
    b.wire(&paid, "out", &via, "record")?;
    b.wire(&order, "json", &done, "record")?;
    b.wire(&via, "out", &done, "value")?;
    b.wire(&done, "out", &save, "value")?;

    // GET /paid/:token: delivers itself to the waiting run, and thanks the payer.
    let pay = b.node("request", &[("method", "GET"), ("path", "/paid/:token"), ("resume", "paid")], [40, 520])?;
    let thanks = text(b, "thanks, the order is paid", [380, 560])?;
    let pay_res = respond(b, OK, "404", [640, 520])?;
    b.wire(&pay, "request", &pay_res, "request")?;
    b.wire(&thanks, "out", &pay_res, "body")
}

fn fetch_json(b: &mut B) -> R {
    let req = b.node("request", &[("method", "GET"), ("path", "/slideshow")], [40, 40])?;
    let path = text(b, "/json", [40, 260])?;
    let call = b.node("http_request", &[("method", "GET"), ("host", "httpbin.org")], [300, 240])?;
    let body = b.node("field", &[("name", "body")], [560, 240])?;
    let parsed = b.node("json_parse", &[], [780, 240])?;
    let show = b.node("field", &[("name", "slideshow")], [1000, 240])?;
    let title = b.node("field", &[("name", "title")], [1220, 240])?;
    let res = respond(b, OK, "404", [1460, 40])?;
    b.wire(&path, "out", &call, "path")?;
    b.wire(&call, "out", &body, "record")?;
    b.wire(&body, "out", &parsed, "text")?;
    b.wire(&parsed, "out", &show, "record")?;
    b.wire(&show, "out", &title, "record")?;
    b.wire(&req, "request", &res, "request")?;
    b.wire(&title, "out", &res, "body")
}

fn sign_payload(b: &mut B) -> R {
    let req = b.node("request", &[("method", "POST"), ("path", "/sign")], [40, 40])?;
    let key = b.node("secret", &[("name", "signing_key")], [40, 300])?;
    let sig = b.node("hmac_sha256", &[("encoding", "hex")], [380, 200])?;
    let b64 = b.node("base64", &[("mode", "encode"), ("alphabet", "standard")], [380, 360])?;
    let empty = b.node("record", &[], [620, 80])?;
    let with_sig = b.node("with_field", &[("name", "signature")], [860, 140])?;
    let with_body = b.node("with_field", &[("name", "body_base64")], [1100, 220])?;
    let res = respond(b, OK, "400", [1340, 40])?;
    b.wire(&key, "out", &sig, "key")?;
    b.wire(&req, "body", &sig, "message")?;
    b.wire(&req, "body", &b64, "text")?;
    b.wire(&empty, "out", &with_sig, "record")?;
    b.wire(&sig, "out", &with_sig, "value")?;
    b.wire(&with_sig, "out", &with_body, "record")?;
    b.wire(&b64, "out", &with_body, "value")?;
    b.wire(&req, "request", &res, "request")?;
    b.wire(&with_body, "out", &res, "body")
}

fn likes(b: &mut B) -> R {
    // A post's count of likes, 0 before the first.
    let count_of = |b: &mut B, key: &str, [x, y]: [i32; 2]| -> R<String> {
        let got = b.node("collection_get", &[("collection", "likes")], [x, y])?;
        let count = b.node("default", &[("value", "0"), ("type", "Number")], [x + 260, y])?;
        b.wire(key, "out", &got, "key")?;
        b.wire(&got, "out", &count, "value")?;
        Ok(count)
    };

    // POST /posts/:id/like: read the count, add one, write it back, answer the new count.
    let like = b.node("request", &[("method", "POST"), ("path", "/posts/:id/like")], [40, 40])?;
    let post = field_of(b, "id", &like, "params", [480, 60])?;
    let one = b.node("const", &[("value", "1"), ("type", "Number")], [740, 200])?;
    let more = b.node("add", &[], [1240, 80])?;
    let keep = b.node("collection_put", &[("collection", "likes")], [1480, 140])?;
    let like_res = respond(b, OK, "400", [1480, 20])?;
    let count = count_of(b, &post, [740, 60])?;
    b.wire(&count, "out", &more, "a")?;
    b.wire(&one, "out", &more, "b")?;
    b.wire(&post, "out", &keep, "key")?;
    b.wire(&more, "out", &keep, "value")?;
    b.wire(&like, "request", &like_res, "request")?;
    b.wire(&more, "out", &like_res, "body")?;

    // GET /posts/:id/likes: the count, 0 before the first like.
    let show = b.node("request", &[("method", "GET"), ("path", "/posts/:id/likes")], [40, 380])?;
    let show_res = respond(b, OK, "404", [1240, 380])?;
    b.wire(&show, "request", &show_res, "request")?;
    let id = field_of(b, "id", &show, "params", [480, 400])?;
    let count = count_of(b, &id, [740, 400])?;
    b.wire(&count, "out", &show_res, "body")?;

    // GET /likes: every post's count, in id order.
    let all = b.node("request", &[("method", "GET"), ("path", "/likes")], [40, 680])?;
    let counts = b.node("collection_list", &[("collection", "likes"), ("limit", "100")], [740, 700])?;
    let all_res = respond(b, OK, "404", [1240, 680])?;
    b.wire(&all, "request", &all_res, "request")?;
    b.wire(&counts, "out", &all_res, "body")
}

fn marketplace(b: &mut B) -> R {
    // The publish key, as callers send it: "Bearer <key>", compared in constant time.
    let authorized = |b: &mut B, req: &str, [x, y]: [i32; 2]| -> R<String> {
        let auth = b.node("field", &[("name", "authorization")], [x, y])?;
        let bearer = text(b, "Bearer ", [x, y + 120])?;
        let key = b.node("secret", &[("name", "publish_key")], [x, y + 220])?;
        let expected = b.node("concat", &[], [x + 260, y + 160])?;
        let matches = b.node("secure_equals", &[], [x + 520, y + 60])?;
        b.wire(req, "headers", &auth, "record")?;
        b.wire(&bearer, "out", &expected, "a")?;
        b.wire(&key, "out", &expected, "b")?;
        b.wire(&auth, "out", &matches, "a")?;
        b.wire(&expected, "out", &matches, "b")?;
        Ok(matches)
    };

    // GET /listings: every listing, in key order (a page at a time, from ?after=).
    let list = b.node("request", &[("method", "GET"), ("path", "/listings")], [40, 40])?;
    let after = b.node("field", &[("name", "after")], [320, 60])?;
    let from = b.node("default", &[("value", ""), ("type", "Text")], [560, 60])?;
    let page = b.node("collection_list", &[("collection", "listings"), ("limit", "200")], [800, 60])?;
    let listed = respond(b, OK, "404", [1060, 40])?;
    b.wire(&list, "query", &after, "record")?;
    b.wire(&after, "out", &from, "value")?;
    b.wire(&from, "out", &page, "after")?;
    b.wire(&list, "request", &listed, "request")?;
    b.wire(&page, "out", &listed, "body")?;

    // GET /listings/:id: one listing, by its id (name@root), or 404.
    let one = b.node("request", &[("method", "GET"), ("path", "/listings/:id")], [40, 300])?;
    let one_id = b.node("field", &[("name", "id")], [320, 320])?;
    let found = b.node("collection_get", &[("collection", "listings")], [560, 320])?;
    let shown = respond(b, OK, "404", [1060, 300])?;
    b.wire(&one, "params", &one_id, "record")?;
    b.wire(&one_id, "out", &found, "key")?;
    b.wire(&one, "request", &shown, "request")?;
    b.wire(&found, "out", &shown, "body")?;

    // POST /listings: publish, with the key. Kept under name@root, with when it was published;
    // no key (or no name and root) and nothing is kept: 401.
    let publish = b.node("request", &[("method", "POST"), ("path", "/listings")], [40, 560])?;
    let publish_ok = authorized(b, &publish, [320, 560])?;
    let name = b.node("field", &[("name", "name")], [320, 900])?;
    let root = b.node("field", &[("name", "root")], [320, 1000])?;
    let at = text(b, "@", [320, 1100])?;
    let name_at = b.node("concat", &[], [580, 940])?;
    let id = b.node("concat", &[], [840, 980])?;
    let clock = b.node("clock", &[], [580, 1100])?;
    let stamped = b.node("with_field", &[("name", "published_at")], [840, 1100])?;
    let with_id = b.node("with_field", &[("name", "id")], [1100, 1040])?;
    let allowed = b.node("guard", &[], [1360, 800])?;
    let keep = b.node("collection_put", &[("collection", "listings")], [1620, 900])?;
    let published = respond(b, OK, "401", [1620, 560])?;
    b.wire(&publish, "json", &name, "record")?;
    b.wire(&publish, "json", &root, "record")?;
    b.wire(&name, "out", &name_at, "a")?;
    b.wire(&at, "out", &name_at, "b")?;
    b.wire(&name_at, "out", &id, "a")?;
    b.wire(&root, "out", &id, "b")?;
    b.wire(&publish, "json", &stamped, "record")?;
    b.wire(&clock, "out", &stamped, "value")?;
    b.wire(&stamped, "out", &with_id, "record")?;
    b.wire(&id, "out", &with_id, "value")?;
    b.wire(&publish_ok, "out", &allowed, "cond")?;
    b.wire(&with_id, "out", &allowed, "value")?;
    b.wire(&id, "out", &keep, "key")?;
    b.wire(&allowed, "out", &keep, "value")?;
    b.wire(&publish, "request", &published, "request")?;
    b.wire(&allowed, "out", &published, "body")?;

    // POST /listings/:id/yank: with the key, flag a version as yanked (kept, never edited: a
    // flow placed from it keeps its hash, and is told).
    let yank = b.node("request", &[("method", "POST"), ("path", "/listings/:id/yank")], [40, 1300])?;
    let yank_ok = authorized(b, &yank, [320, 1300])?;
    let yank_id = b.node("field", &[("name", "id")], [320, 1640])?;
    let was = b.node("collection_get", &[("collection", "listings")], [580, 1640])?;
    let yes = b.node("const", &[("value", "true"), ("type", "Boolean")], [580, 1760])?;
    let flagged = b.node("with_field", &[("name", "yanked")], [840, 1680])?;
    let may_yank = b.node("guard", &[], [1100, 1500])?;
    let rewrite = b.node("collection_put", &[("collection", "listings")], [1360, 1600])?;
    let yanked = respond(b, OK, "401", [1360, 1300])?;
    b.wire(&yank, "params", &yank_id, "record")?;
    b.wire(&yank_id, "out", &was, "key")?;
    b.wire(&was, "out", &flagged, "record")?;
    b.wire(&yes, "out", &flagged, "value")?;
    b.wire(&yank_ok, "out", &may_yank, "cond")?;
    b.wire(&flagged, "out", &may_yank, "value")?;
    b.wire(&yank_id, "out", &rewrite, "key")?;
    b.wire(&may_yank, "out", &rewrite, "value")?;
    b.wire(&yank, "request", &yanked, "request")?;
    b.wire(&may_yank, "out", &yanked, "body")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_sample_builds_with_a_unique_name_and_port() {
        let names: HashSet<_> = samples().iter().map(|s| s.name).collect();
        assert_eq!(names.len(), samples().len());
        let ports: Vec<_> = samples().iter().filter_map(|s| s.port).collect();
        assert_eq!(ports.iter().collect::<HashSet<_>>().len(), ports.len(), "each server its own port");
        for s in samples() {
            assert!(s.port.is_some() != s.interval.is_some(), "{}: a port or an interval", s.name);
            let doc = build(s).unwrap_or_else(|e| panic!("{e}"));
            assert!(!doc.nodes.is_empty(), "{}", s.name);
        }
    }

    #[test]
    fn a_holon_whose_ports_dont_match_says_so() {
        let mut fb = FlowBuilder::new();
        let mut b = B(&mut fb);
        let err = b.holon("h", |i| i.node("lowercase", &[], [0, 0]).map(drop), &["a", "b"], &["c"]).unwrap_err();
        assert!(err.starts_with("holon \"h\" has inputs"), "{err}");
    }

    #[test]
    fn a_misspelt_leaf_param_or_port_names_the_sample() {
        let broken = |build: fn(&mut B) -> R| build_err(Sample { build, ..sample("hello-api").unwrap().clone() });
        fn build_err(s: Sample) -> String {
            build(&s).unwrap_err()
        }
        assert_eq!(
            broken(|b| {
                let (f, c) = (b.node("field", &[], [0, 0])?, b.node("concat", &[], [0, 0])?);
                b.wire(&f, "nope", &c, "a")
            }),
            r#"sample "hello-api": field has no output "nope""#
        );
        assert!(broken(|b| b.node("fieldd", &[], [0, 0]).map(drop)).contains(r#"no leaf "fieldd""#));
        assert!(broken(|b| b.node("field", &[("nam", "x")], [0, 0]).map(drop)).contains(r#"has no param "nam""#));
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
