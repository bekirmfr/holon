# Translation at scale: what Holon needs to take in nodes from any platform (L4)

**Status:** design, 2026-11-06, against `77f91a7`. Nothing here is built. It builds on two notes:
[n8n import](n8n-import-L4.md) (the survey, the five tiers, the `dexpaprika` proof) and
[host capabilities](host-capabilities-L4.md) (the doors, and §7's plan for building them). Claims
about Holon are cited by file. Claims about other platforms come from their own public docs and
source, read only, nothing installed or run, each cited by URL. Where those didn't confirm a
claim, this note says so.

The n8n note asked how much of one catalog can become Holon text. This one asks what Holon needs
so that nodes from **any** platform can: n8n, Zapier, Pipedream, Activepieces, Make, Node-RED,
OpenAPI documents, MCP servers, Airbyte connectors.

> **A foreign node reduces to Holon when four things hold:**
>
> 1. its interface maps to ports;
> 2. its computation runs in a pure engine;
> 3. every effect goes through a Holon door;
> 4. the translation can be shown to behave the same.
>
> Taking in nodes at scale means growing Holon along those four lines, plus one pipeline that
> every platform feeds.

**Why mass translation is safe at all.** A flow's permissions are derived from its text, not
declared by whoever wrote it: `capabilities_of` gives each leaf's, and `capabilities` takes the
union over a unit's closure (`crates/holon-core/src/kind.rs`). So a
translation, whether a tool or an agent wrote it, can do only what its own text visibly asks for:

- it calls the hosts named in its `http.*` literals;
- it sends each secret only where the approval says ([secret destinations](secret-destinations-L4.md));
- it touches only the folders, databases and channels it names;
- it runs only within the budgets it declares.

A wrong translation can misbehave only inside doors it visibly declares. That bound is what lets
a person approve a thousand imported nodes without reading a thousand nodes' code.

It is a bound, not correctness. Inside its doors, a wrong translation can still send the wrong
request to a granted host, with a granted secret: delete where it should list. Two things narrow
that:

- the approval's grain, which is the host-capabilities rule R3b: consent at the grain the
  question needs;
- the differential harness (§6), which is evidence, not proof.

---

## 1. The four conditions, and where nodes fail them today

| Condition | What carries it in Holon today | Where the n8n survey's nodes fail it | Enhancement |
|---|---|---|---|
| Interface → ports | holon inputs and outputs, records, absence ([LANGUAGE](../LANGUAGE.md) §5.8, §6.3) | parameters with defaults and conditions (`displayOptions`) translate by hand as `??` and `when` | §3 vocabulary |
| Computation → a pure engine | `js` holons: Boa in a wasm module with no imports, a fresh realm a call ([js node](../engineering/capabilities/js-node.md)) | synchronous only. No `async`, no pause on a request, so code that calls out can't run (62% of top-150 downloads) | §4 engines |
| Effects → doors | `http.*`, `secret.*`, folders, databases (SQLite, Postgres, MySQL, MongoDB), channels, clocks | bytes (binary in 30 of 72 helpers-only packages, 6 of 25 declarative nodes), OAuth2 code flow, webhook registration, mail, SFTP, S3, raw sockets, programs | §2 doors |
| Shown to behave the same | by hand: `dexpaprika`, run live against n8n's behaviour written down ([n8n import](n8n-import-L4.md) §11) | nothing mechanical | §6 harness |

Two more needs cut across the four. One pipeline turns any source into Holon text (§5). A place to
keep what was translated, with its provenance (§7).

## 2. Doors

### 2.1 Planned already

Each of these is a step of [host capabilities](host-capabilities-L4.md) §7, designed in its own
section there. Translation adds no new door here, only demand:

| Door | Step | What it unlocks for imported nodes |
|---|---|---|
| **Bytes** (`Bytes`, and `Decimal` with it) | 1.1, §5.10 | Every binary node: file uploads and downloads, audio, images, PDFs. 30 of the 72 helpers-only packages handle binary data, and 6 of the 25 declarative nodes do. The most-demanded door, and first in Tier 1 already. |
| **Formats and dates** | 1.2, §5.1 | Date parameters and formatting. 24 of 72 helpers-only packages read the clock. Also CSV, XML, zip. |
| **Email** | 1.3, §5.2 | `n8n-nodes-imap` (imapflow, nodemailer) and every "send an email" node that isn't an HTTP API. |
| **Remote folders** | 1.4, §6 | `n8n-nodes-minio` (S3) and the SFTP half of `n8n-nodes-sshv2`. |
| **Feeds and queues** | 1.6, §5.5, §5.9 | Streaming and queue triggers (SSE, WebSocket, one queue engine). |
| **Commands** | 1.7, §5.7 | Nodes that run a program, when an admin binds it (§4.2). |
| **A sandbox for agents** | 2.4, §5.8 | Nodes whose program can't be bound as a command (§4.2). |
| **MQTT, more queue engines, gRPC streams** | Tier 3 | Node-RED's core value: devices and brokers. |

### 2.2 New: OAuth2 for any API

**What exists.** The client-credentials grant is covered: a flow posts to the token endpoint
itself (sample `client-credentials`). The authorization-code grant isn't. A person must consent in
a browser, and the refresh token must be kept and used to mint access tokens. Host capabilities
has this only as fork 2, for mailboxes, and in Tier 3 ("Mailboxes through OAuth").

**Why it weighs more than the community survey says.** 18 of 168 community credentials are
OAuth2. That undercounts the head of the market, which n8n wrote itself: community packages
mostly fill the gaps n8n's own nodes leave. n8n's built-in package, `n8n-nodes-base` 2.15.1, was read as text
for counting only (never translated, never shipped):

- **100 of its 391 credentials are OAuth2.** 61 default to the authorization-code grant, 3 to
  PKCE and 1 to client credentials. 35 inherit their grant from a parent such as
  `googleOAuth2Api`.
- **39 integrations accept only OAuth2.** These include:
  - Google: Calendar, Analytics, Ads, Contacts, YouTube, Business Profile;
  - Microsoft: Outlook, Excel, Teams, OneDrive, SharePoint, Dynamics, To Do;
  - QuickBooks, Xero, LinkedIn, Zoho, Box, Reddit, Spotify.
- **54 more offer OAuth2 beside a key or a service account.** These include Google Sheets, Drive,
  Gmail and Docs; Salesforce, HubSpot, Slack, Notion, Shopify, Zendesk, Zoom, GitHub and GitLab.
  The three generic nodes that take any credential (HTTP Request, GraphQL, Evaluation) aren't
  counted. For Google's, the other choice is a service account. Its JWT must be signed with a
  private key, which is host-capabilities R3c's case: the key is the host's to hold (host
  capabilities §5.3).

That is 93 of about 360 integration folders (the count groups nodes by folder): about a quarter
of mainstream SaaS, including the office suites. For the payment-ops job M3 aims at, the
accounting systems on that list, QuickBooks and Xero, are reachable only through it.

Every platform §5.2 reads holds OAuth2 for its nodes. Zapier calls the app's refresh, Make runs
the app's `refresh` block, and Pipedream and Activepieces keep the refresh token themselves.
Activepieces strips it before a piece ever sees the connection. A node translated from any of
them assumes a host that does the same.

**The door, sketched.** It is a host capability under R3c: the security step is the host's to
hold.

- **A connection is a secret of a new kind.** It names the authorization and token endpoints, the
  client (its id, and its secret as an ordinary secret) and the scopes it asks for.
- **Consent in the editor.** An admin connects: the editor opens the provider's consent page, and
  the server's own callback takes the code, with PKCE.
- **The host refreshes.** It keeps the refresh token sealed, as secrets are, and never lets it
  enter a run. It mints access tokens before they expire, and again after a `401`.
- **A run sees `secret.x`:** a current access token, tainted like any secret. Where it may be sent
  is a destination, as now.
- **The approval adds two lines:** the scopes ("read your calendars"), and the token endpoint's
  host.
- **Replay doesn't change:** runs that read a secret aren't re-run by `holon verify`, as today.
- **A refresh is a host act,** recorded in the activity log, not in a run.

**Where it goes in the plan:** its own Tier 1 step. It needs nothing earlier; email (1.3) reaches
Microsoft 365 and Gmail through it, which settles fork 2; and translation needs it more than any
door but bytes.

### 2.3 New: calls at deploy and undeploy

Webhook triggers register themselves with the service:

- n8n's `webhookMethods` (`checkExists`, `create`, `delete`);
- Zapier's `performSubscribe` and `performUnsubscribe`;
- Pipedream's `hooks.activate` and `deactivate`;
- Activepieces' `onEnable` and `onDisable`;
- Make's `attach` and `detach` (each platform as §5.2 cites it).

72 of the 110 trigger nodes in `n8n-nodes-base` do. So do nodes in 26 of the 148 top community
packages and 14 of the random 60. Holon has a route for the webhook to arrive at, but no moment to
register it.

**The construct:** `on deploy { … }` and `on undeploy { … }` handlers, each run once:

- `on deploy` runs when a version starts serving, before traffic is counted as served;
- `on undeploy` runs when the version stops, while its grant still holds.

They are ordinary handlers. Their requests are in the manifest like any other, approved with the
flow, and recorded as runs. What a registration returns (the webhook's id) is kept in the flow's
state, for the undeploy to delete.

**What it needs beside the handlers:**

- **The server's public URL,** to tell the service where to send. That is a server feature an
  operator sets: Holon's address seen from outside, which it can't know itself.
- **A rule for a failed registration:** fork 2.

A new version re-runs `on deploy`. Registration must be idempotent, as `checkExists` makes it in
n8n.

### 2.4 Multipart

[Host capabilities](host-capabilities-L4.md) §5.13 designs the **incoming** direction: a leaf over
`Bytes` that splits a browser's `multipart/form-data` into parts, unscheduled.

**Outgoing** multipart isn't designed anywhere. Uploading a file to an API is how most binary
nodes send (OpenAI files, Telegram documents, Slack files). `http_upload` sends one file by handle
as the whole body. Outgoing multipart is a leaf (R1) that builds a body from parts (text fields,
and `Bytes` or file handles), after step 1.1. It is new here.

## 3. Vocabulary for generated code

Generated text needs a few leaves and constructs that hand-written flows get by without.

**Leaves (R1, one catalog entry each):**

| Leaf | Today | Unlocks |
|---|---|---|
| `url_encode` (and a query string from a record) | none: `dexpaprika` spends its one `js` holon on it | every `send: query`, 16 of 25 declarative nodes |
| `base64`, `hex` over bytes | over text only (the catalog's `base64`, `hex`) | binary bodies, once step 1.1 lands |
| list `sort` and `filter` | `fold` with `when` filters; nothing sorts | `postReceive: filter` / `sort`, most `postReceive` functions |
| a field at a path given as text | **covered:** `lookup(record, key)` takes a key from a wire, and a dotted path is a `fold` of `lookup` over `split(path, ".")` | `rootProperty` and dot paths that a parameter holds. A generator writes the fold; no leaf is needed. |

**Constructs:**

- **Inputs with defaults, and conditional inputs:** n8n's `displayOptions`, Zapier's
  `altersDynamicFields`, Pipedream's `reloadProps`, Activepieces' `DynamicProperties`, Make's
  `nested` (§5.1). Lowers to `??` and `when`, which exist. The construct
  only spells them at the holon's head, so a reader of the text sees the defaults as the source
  had them.
- **Bounded pagination:** request, next cursor, stop when done or after N pages. Lowers to a
  `fold` over `range(n)` whose step asks only while the last page said there is more. The choice
  is lazy, so a page not needed is never asked for, and each page is a round ([n8n
  import](n8n-import-L4.md) §5). The bound shows in the approval as a budget, within
  `max_requests`. Needed by 3 of 25 declarative nodes and 18 of 72 helpers-only packages.
- **Retry and backoff as declared policy,** instead of `sleep` (timers in 4 of 72 packages).
  **This one doesn't lower to rounds.** A round has no delay, so a wait between attempts is
  either the run's time or a durable `await`. The honest form is a param of the request leaf
  (`retry: 3`), which the host honours inside `perform`, within `run_budget`. Each attempt is
  recorded, and the run sees the last answer. It asks for no new trust: the same host and the
  same request. Only idempotent methods may retry (`GET`, `HEAD`, `PUT`, `DELETE`), and others
  only when they carry an idempotency key. Otherwise one `POST` could land twice.

Every item but retry lowers to mechanisms core already has, with no new trust. Retry is a host
behaviour on an existing door.

## 4. Engines that ask for effects

### 4.1 The contract, generalized

Today a module is `engine(source, args) -> {value} | {error}`, synchronous and closed ([js
node](../engineering/capabilities/js-node.md) §2). [n8n import](n8n-import-L4.md) §7 asked for a
pause on an HTTP request. The general form is:

> **A call may end pending, with an effect record, for any door. The answers so far come in on the
> next round.**

- An effect record is what the matching leaf would have been: `{door: "http", method, host, path,
  headers, body}`, `{door: "file_read", folder, name}`, `{door: "db_find", …}`.
- The host checks it by **the same rules as that leaf**: the grant, the destinations, the limits.
  It performs and records it between rounds, as it does for `http_request` now. The answers are
  keyed by node path and request hash ([outbound requests](../engineering/capabilities/outbound-requests.md)
  §2).
- The engine stays a pure function of its inputs and the answers so far, so `holon verify`
  replays it from the log.
- The doors a module may ask for are declared on its node and shown in the approval, as a
  translated flow's would be: "runs code it carries; may call <hosts> with <secrets>; may read
  folder <name>".
- `Promise`, `async` and `await` settle within a call: microtasks run to the end. An awaited,
  unanswered effect ends the call as pending.

### 4.2 Shims of standard I/O onto doors

A shim is a small library put in the realm that turns a platform's I/O call into effect records:

| Shim | Becomes |
|---|---|
| n8n's `this.helpers.httpRequest*`, `requestWithAuthentication` | `http` |
| Zapier's `z.request` | `http` |
| Pipedream's `$` and `axios` from `@pipedream/platform` | `http` |
| `fetch`, axios, Node's `http(s).request`, undici | `http` |
| `fs` inside a bound folder (`readFile`, `writeFile`, `readdir`, `existsSync`) | `file_*` on a named folder |

A shim never reaches past the doors. A `require` the shims don't cover is refused.

**The 31 top-150 community packages refused for their own I/O, re-sorted under these shims**
(tier 5 in [n8n import](n8n-import-L4.md) §8; downloads are monthly, as a share of the top 150's
10.5 million):

| Under the shims | Packages | Operations | Downloads | Which |
|---|---|---|---|---|
| **HTTP shims → tier 4** | 7 | 475 | 4.7% | `n8n-nodes-mcp` (4.5% alone: its HTTP and SSE transports; the stdio transport starts a program), gohighlevel, wecom, wts, 2kw, fishaudio, mcp-server-with-mcp-events |
| **HTTP and `fs` in a bound folder → tier 4** | 3 | 632 | 1.1% | autotask (596 operations; it also keeps a Redis cache), browserflow, nna-telegram-sender |
| **A door Holon has or plans** | 6 | 19 | 0.6% | imap (email, 1.3), minio (S3, 1.4), sshv2 (SFTP, 1.4; its remote commands, Tier 3), mongo-bulk (MongoDB, which exists), firebird (no engine), message-debounce (a raw socket to Redis; no engine) |
| **A program** | 5 | 78 | 0.2% | hollycrm (pandoc), claude-cli-bridge (a CLI): a bound command (1.7); kordoc (PowerShell), software-teams, actual (the Actual Budget API's local files): the sandbox (2.4) |
| **An AI sub-node** | 5 | 5 | 0.5% | openai-advanced, langfuse, kimi, zhipu, merge: each hands n8n's agent a live LangChain model object. No Holon counterpart. What it does is a model call, which translates as one, by reading. |
| **Still refused** | 5 | 14 | 1.5% | puppeteer, playwright, whatsapp (browsers); tesseractjs (OCR, workers, downloaded data); webpage-content-extractor (jsdom) |

So the HTTP and `fs` shims move 10 of the 31 packages (5.8% of downloads, 1,107 operations) from
refused to interpreted. Another 6 wait on doors already planned or present. The five obfuscated
packages stay out: a shim could run them, but nobody could read what they do.

The random 60 from ranks 151 to 1,000 sort alike:

- HTTP shims: 5 of 14;
- AI sub-nodes: 6, more than at the head;
- a program: 1;
- a socket: 1;
- code built and run with `vm`: 1.

**Bundled dependencies.** A shimmed node carries its dependencies into the realm. LangChain or an
SDK is hundreds of KB to parse, and makes §4.3 matter.

### 4.3 Realm snapshots keyed by the code's hash

A node's code is a median 81 KB (p75 191 KB) before its dependencies, and today every round
parses it again in a fresh realm. A snapshot takes the realm after the code has loaded, before
any call, and restores it on each call. Determinism holds:

- the snapshot is a pure function of the engine and the code;
- the prelude has no clock or randomness to leak in;
- it is keyed by the code's hash, as the unit is.

The known technique is pre-initializing a wasm module: run its start, then write its memory into
a new module (Wizer, Bytecode Alliance). That makes the snapshot itself a module with no imports,
content-addressed, so it stays a unit and core's purity check still holds. **Unconfirmed:** that
Boa's state survives such a snapshot. It wants a spike before anything rests on it.

### 4.4 Python as the next engine

It is the same contract with another engine module. Who needs it:

- Airbyte's custom components and its Python connectors (§5.2);
- Pipedream's Python steps, and n8n's Code node in Python.

The connectors of the platforms §5.2 reads are JavaScript or TypeScript, or declarative. Python is
for code those platforms let users write, and for Airbyte.

**Unconfirmed:** an interpreter that builds to wasm with no imports. RustPython is pure Rust, as
Boa is. CPython's WASI build imports WASI's clock, random and files, which would have to be
stubbed out as Boa's clock was. Either wants a spike.

**Measured against §8's step 4 first.** If agents translate code into text that passes the
harness, an engine buys less than it costs.

## 5. One pipeline, one intermediate form

### 5.1 The form

Every platform below describes a node with the same few parts, under its own names. One record
holds them. Thin readers fill it, one per source, and one back end turns it into Holon text. The
record is the importer's working form, kept beside the output as provenance and never deployed.

| Part | What it holds | n8n · Zapier · Pipedream · Activepieces · Make |
|---|---|---|
| **Source** | platform, package and version, the file and its hash, license, attribution | `package.json` · the app's `package.json` · the component's `key` and `version` · the piece's npm package · the app's base |
| **Parameters** | name, type, required, default, static options, the condition it is shown under | `properties`, `displayOptions` · `inputFields`, `altersDynamicFields` · `props`, `reloadProps` · `props`, `DynamicProperties` · `parameters`, `nested` |
| **Credentials** | the kind (key, basic, bearer, custom, OAuth2 with its endpoints, grant and scopes); each field a secret or a value asked at import; where it goes in a request; the test request | `credentials`, `authenticate`, `test` · `authentication` (`basic`, `custom`, `digest`, `oauth1`, `oauth2`, `session`) · an `app` prop, `$auth` · `PieceAuth` (`SecretText`, `BasicAuth`, `CustomAuth`, `OAuth2`) · connections (`basic`, JWT, OAuth 1 and 2) |
| **Request templates** | method; the host and where it comes from (a literal, the credential, a parameter); path, query, headers, body; pagination; what to take from the answer | `routing` · the shorthand `perform: {url, method, params, …}` · none · none (only the generic `createCustomApiCallAction`) · `communication` |
| **Code pieces** | language, role (an expression, a hook before or after a request, a whole operation), text and hash, and its I/O class: computes only, calls through the platform's helper, or does I/O of its own (naming which) | `preSend`, `postReceive`, `={{ }}`, `execute` · a `perform` function, `z.request` · `run`, `methods`, `axios($, …)` · `run`, `httpClient` · IML functions (JS, on request) |
| **Lifecycle** | per call; a poll (interval, dedupe key); a webhook (its route, subscribe and unsubscribe, handshake, renewal); long-lived | `poll`, `webhookMethods` · polling with `id` dedupe, `performSubscribe` / `performUnsubscribe` · timer sources, `hooks.activate` / `deactivate` · `pollingHelper`, `onEnable` / `onDisable`, `onRenew` · `trigger`, `attach` / `detach` |

**The back end** assigns each piece a tier by [n8n import](n8n-import-L4.md) §4's rules:
translate only what is provably the same, and interpretation is the reference semantics. It
writes fmt-canonical Holon text with the provenance in its header comment, and keeps the record
for §7. A reader's work is mapping names into the record. The tiers, the text and the checks are
written once.

### 5.2 The sources

Each platform's format and license were read from its own docs or source on 2026-11-06. The
counts were taken the same day from public listings.

| Source | Declarative | Code does I/O through | OAuth2 held by | Subscribe hooks | Catalog license | Public | Size |
|---|---|---|---|---|---|---|---|
| n8n community | 1 package in 6 (19% of downloads) | `this.helpers` | n8n | `webhookMethods` | MIT (147 of 148) | yes, npm | 14,227 packages |
| OpenAPI | all: it only describes | — | — (it names `oauth2` flows) | `callbacks`; `webhooks` from 3.1 | the spec Apache-2.0; each document its owner's | many; APIs.guru lists some | APIs.guru: 2,529 APIs, 3,992 specs |
| Airbyte | 509 of 592 sources | the CDK; `Custom*` Python classes | the connector (`OAuthAuthenticator`, refresh) | none: batch syncs | ELv2 554, MIT 32, enterprise 6 | yes | 592 sources |
| MCP | the interface only | the server | OAuth 2.1, HTTP transports | none | the spec Apache-2.0 | registry | about 57,500 servers |
| Zapier | the request shorthand; share unmeasurable | `z.request` | Zapier | `performSubscribe` | Platform Agreement | no | 10,453+ claimed |
| Make | all | `communication` | Make | `attach` / `detach` | unconfirmed | no | 3,500+ claimed |
| Activepieces | none | `httpClient`, SDKs | the server | `onEnable` / `onDisable` | MIT outside `ee/` | yes | 773 pieces |
| Pipedream | none | `axios`, any npm, `/tmp` | Pipedream | `hooks.activate` / `deactivate` | Source Available 1.0 | yes | 3,401 apps |
| Node-RED | none | any Node I/O | — | — (long-lived) | Apache-2.0 core; packages vary | yes | 6,362 listed |

**n8n community nodes** are [n8n import](n8n-import-L4.md)'s subject. Two more facts come from
n8n's own docs:

- the declarative style is the default for new nodes, while triggers, GraphQL and external
  dependencies need the programmatic style
  ([choose a method](https://docs.n8n.io/integrations/creating-nodes/plan/choose-node-method/));
- a verified community node must be MIT and have no external dependencies
  ([verification](https://docs.n8n.io/integrations/creating-nodes/build/reference/verification-guidelines/)).

So a verified node does its I/O through n8n's helpers or a Node built-in, never through a bundled
SDK. It is tier 4, or tier 4 under a shim (§4.2), unless it opens sockets or starts programs.

**OpenAPI documents.**

- **Format.** A description of operations: paths, parameters (`in`: path, query, querystring,
  header, cookie; path parameters always `required`), bodies by schema, and `securitySchemes`.
  The schemes are `apiKey`, `http`, `mutualTLS`, `oauth2` with its flows, and `openIdConnect`
  ([OAS](https://spec.openapis.org/oas/latest.html), 3.2.1).
- **Defaults.** A schema's `default` documents what the receiver does. It isn't a value to send,
  so a translation mustn't send it.
- **Callbacks.** `callbacks` and, from 3.1, `webhooks` describe requests the API makes back: a
  route on Holon's side.
- **Reader.** Feeds tiers 1–2 directly, with no code at all
  ([n8n import](n8n-import-L4.md) §10). `mutualTLS` needs step 1.5's client identities, and
  `oauth2`'s code flow needs §2.2.
- **License.** Each document carries its owner's terms. APIs.guru's directory is CC0 for what was
  contributed to it, and the rest was gathered "under the Fair use principle"
  ([README](https://raw.githubusercontent.com/APIs-guru/openapi-directory/main/README.md)). So a
  translation follows the document's own terms, checked per API.

**Airbyte declarative connectors.**

- **Format.** A YAML manifest checked against the declarative component schema
  ([schema](https://raw.githubusercontent.com/airbytehq/airbyte-python-cdk/main/airbyte_cdk/sources/declarative/declarative_component_schema.yaml)).
  It is made of:
  - an `HttpRequester` (`url_base`, `path`, `http_method`, parameters, headers, JSON body, an
    authenticator, an error handler);
  - paginators (`CursorPagination`, `OffsetIncrement`, `PageIncrement`) and record selectors;
  - incremental cursors (`DatetimeBasedCursor`);
  - backoff strategies (constant, exponential, wait from a header).

  Strings are Jinja templates over `config`, `response` and `next_page_token`, with macros such
  as `now_utc` and `format_datetime`
  ([interpolation](https://docs.airbyte.com/platform/connector-development/config-based/advanced-topics/string-interpolation)).
  Thirteen `Custom*` types name a Python class.
- **What each part becomes:**
  - the requester, tier 1;
  - Jinja, tier 2 where it is a reference, and dates where it formats (step 1.2);
  - the paginators, §3's pagination;
  - the backoff, §3's retry;
  - an incremental cursor, a state cell on a clock: an Airbyte stream is a sync, `every` pulling
    pages into a collection;
  - custom Python, §4.4 or refused.
- **Catalog.** The registry (`oss_registry.json`, counted 2026-11-06) holds 592 sources. Of
  these, 509 are `manifest-only` and 527 carry `cdk:low-code`. That makes it **the most
  declarative public catalog**: 86% of its sources are a manifest with no code beside it, unless
  it names a custom class. n8n's community has more declarative packages in number (about one in
  six of 14,227), but most of those carry functions.
- **License.** 554 sources are ELv2 and 32 MIT, 28 of the MIT ones manifest-only. ELv2 grants
  derivative works and distribution, but "you may not provide the software to third parties as a
  hosted or managed service"
  ([LICENSE](https://raw.githubusercontent.com/airbytehq/airbyte/master/LICENSE)).
  - Self-hosted Holon shipping translations with their notices fits.
  - A hosted Holon offering them is a question for counsel (§9, fork 11).
  - Airbyte's docs and its repositories' license files disagree on the CDK: the docs say ELv2,
    and the CDK repository's own file says MIT. Another reason to keep each connector's own
    `license` field in its provenance.

**MCP tool definitions.**

- **Format.** A tool is `name`, `inputSchema` (JSON Schema), `outputSchema` and `annotations`
  (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`), spec 2026-07-28
  ([tools](https://modelcontextprotocol.io/specification/latest/server/tools)). The server does
  the work, so the schema gives **ports and nothing else**.
- **Reader.** A tool becomes a holon whose body calls the server's `tools/call` over HTTP: tier
  1, with the server's host as the permission.
- **What the approval can't say.** Every tool sits behind one host, so "call
  https://mcp.example.com" approves `delete_repo` along with `list_repos`. That is too coarse by
  host-capabilities R3b (§9, fork 12).
- **Annotations are hints,** and the spec says not to trust them from servers one doesn't trust.
  So a translation may show them, never grant on them.
- **Auth.** HTTP transports authorize with OAuth 2.1, PKCE and resource indicators
  ([authorization](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization)):
  §2.2 again. Stdio servers are programs (§4.2: a bound command).
- **Catalog.** The official registry pages to about 57,500 servers, latest versions only
  (counted 2026-11-06).

**Zapier platform apps.**

- **Format.** An app's triggers, searches and creates each have a `perform`. Its schema allows
  either a function or a request shorthand: `{url, method, params, body, headers}` with
  `{{curlies}}` templates
  ([schema](https://raw.githubusercontent.com/zapier/zapier-platform/main/packages/schema/docs/build/schema.md)).
  The UI builder's Form Mode is that shorthand; Code Mode is a function, and the two can't be
  mixed in one step ([form mode](https://docs.zapier.com/integrations/build/form-mode.md)). Code
  makes requests through `z.request`
  ([requests](https://docs.zapier.com/integrations/build-cli/making-http-requests.md)).
- **Auth and lifecycle.** OAuth2 with the code grant is built in; Zapier refreshes by calling the
  app's own `refreshAccessToken`. Webhook triggers subscribe through `performSubscribe` and
  `performUnsubscribe`. Polling dedupes on `id` by default.
- **Reader.** A good one: Form Mode steps are tier 1–2 as they stand.
- **Catalog.** Not public. No repository of apps was found, though "not public" is
  **unconfirmed** as a blanket claim. The platform repository's LICENSE binds anyone who
  downloads or uses it to the Zapier Platform Agreement
  ([LICENSE](https://raw.githubusercontent.com/zapier/zapier-platform/main/LICENSE)). So a
  Zapier reader serves an app's own developer bringing their app across: they hold its source.
  Holon doesn't build a catalog from it.
- **Size.** Zapier claims 10,453+ apps ([apps](https://zapier.com/apps)). Their declarative
  share can't be measured.

**Pipedream components.**

- **Format.** A component is an `.mjs` module (`props`, `methods`, `hooks`, `dedupe`, `run`) with
  no declarative form
  ([component API](https://pipedream.com/docs/components/contributing/api)). Requests go through
  `axios` from `@pipedream/platform`, but any npm package may be required, an app's own SDK is
  recommended, and `/tmp` is writable
  ([guidelines](https://pipedream.com/docs/components/contributing/guidelines.md)).
- **Auth and lifecycle.** OAuth is Pipedream's: it holds the refresh token and keeps the access
  token fresh ([connected accounts](https://pipedream.com/docs/apps/connected-accounts.md)).
  Sources subscribe in `hooks.activate` and unsubscribe in `deactivate`.
- **Catalog.** `components/` is public: 3,401 app folders, holding 12,149 actions and 3,392
  sources ([components](https://github.com/PipedreamHQ/pipedream/tree/master/components)).
- **License.** It falls under the repository's Pipedream Source Available License 1.0. That
  grants derivative works but excludes "any commercial use", including offering a competing
  service ([LICENSE](https://raw.githubusercontent.com/PipedreamHQ/pipedream/master/LICENSE)).
  So, like n8n's own nodes, Pipedream's catalog isn't one Holon translates and ships.
- **Reader.** Would serve a user's own components, and is code-first: tier 4 under the `axios`
  shim (§4.2), or an agent reading.

**Activepieces pieces.**

- **Format.** TypeScript: `createPiece`, `createAction`, `createTrigger`. There is no
  declarative request form. The one shortcut is `createCustomApiCallAction`, a generic "custom
  API call" with a `baseUrl` and auth mapping
  ([custom API calls](https://www.activepieces.com/docs/build-pieces/piece-reference/custom-api-calls.md)).
  Requests go through `httpClient.sendRequest`, whose request carries `retries` and
  `authentication`. Pieces also use npm SDKs, and core pieces include `sftp` and `smtp`.
- **Auth.** OAuth2 (code grant, and client credentials) is built in. The server refreshes the
  token and strips the refresh token and client secret before a piece sees the connection.
  That is the shape §2.2 proposes.
- **Lifecycle.** Triggers poll with a dedupe strategy (time-based or last item) or take
  webhooks, with `onEnable` and `onDisable` (plus `onHandshake` and `onRenew`).
- **License.** MIT outside `packages/ee/`; the pieces live outside it
  ([LICENSE](https://raw.githubusercontent.com/activepieces/activepieces/main/LICENSE)).
- **Catalog.** 745 community pieces and 28 core
  ([community](https://github.com/activepieces/activepieces/tree/main/packages/pieces/community)).
  How many use only `httpClient` wasn't measured: it means reading all 745 as the n8n survey
  read its 150.
- **Reader.** After n8n, the **most open code-first catalog**. Its `httpClient` is one more HTTP
  shim, and its pieces are what agent translation (§8, step 4) would take on first.

**Make custom apps.**

- **Format.** Fully declarative: JSON with IML `{{…}}` expressions. A module's `communication`
  holds `url`, `method`, `qs`, `headers`, `body`, `pagination` and `response` (`output`,
  `iterate`, `trigger`)
  ([communication](https://developers.make.com/custom-apps-documentation/component-blocks/api.md)).
  JavaScript appears only in custom IML functions: off by default, built-ins only, 10 s and
  5,000 characters
  ([IML functions](https://developers.make.com/custom-apps-documentation/app-components/iml-functions.md)).
- **Auth and lifecycle.** OAuth2 is the app's own `authorize`, `token` and `refresh` blocks,
  which Make runs. Webhooks subscribe with `attach` and `detach`.
- **Reader.** The cleanest fit for tiers 1–2: everything is a request template.
- **Catalog.** Not public. Make claims 3,500+ apps
  ([integrations](https://www.make.com/en/integrations)). No license for custom apps was found
  (**unconfirmed**). So, as with Zapier, a Make reader serves an app's own developer.

**Node-RED nodes.**

- **Format.** A JavaScript constructor registered with `RED.nodes.registerType`, plus an HTML
  file for its editor ([creating nodes](https://nodered.org/docs/creating-nodes/node-js)).
  - The runtime gives inbound HTTP (`httpNode`) but no documented outbound helper. Whether
    custom nodes have one is **unconfirmed**.
  - Nodes are long-lived: `on('input')` for each message, `on('close')` at the end.
  - Credentials are `text` or `password` fields, kept apart from the flow.
- **License.** Apache-2.0 for the core
  ([LICENSE](https://raw.githubusercontent.com/node-red/node-red/master/LICENSE)). Contributed
  packages carry their own.
- **Catalog.** The flow library lists 6,362 nodes (from an undocumented JSON listing). npm's
  `node-red` keyword gives 7,743.
- **Reader.** Node-RED's value is protocols and devices: serial, MQTT, Modbus, sockets. There is
  no declarative form and no common I/O call to shim, so a reader is an agent's. Most nodes wait
  on Tier 3 doors (MQTT, sockets) or are refused (§10). It is the last reader to build.

## 6. A differential harness

What lets an agent translate a code node by reading, into tier 1–3 text, and lets anyone check it
afterwards.

- **A stand-in server** takes every request and records it: method, host, path, query, headers
  and body, with each secret replaced by a marker naming it. It answers from fixtures: recorded
  or synthesized answers, the same for both sides.
- **The original node runs in its own runtime, in a sandbox,** at import or in CI, never in
  production. Its only network is the stand-in. Its secrets are test values.
- **The translation runs in Holon** against the same stand-in, through the same doors.
- **Inputs are generated from the parameter schema:** each default, each option of a list, each
  conditional field shown and hidden, optional fields left out, and boundary values (empty text,
  a `/` or `%` in an id, a long list).
- **Compared:** the requests each side sent, in order where order matters, and the outputs. A
  difference fails the import, naming the input and the first request that differs.

**It is evidence, not proof.** It covers the inputs it tried and the answers the fixtures gave. A
node that branches on an answer the fixtures never give is untested there. **Permissions don't
rest on it:** they come from the translated text whatever the harness says, so a translation that
passed for the wrong reasons can still do only what its text visibly asks for.

What runs the original is fork 5.

## 7. Packaging and a registry

**Where an imported node lives today.** Copied, as a `.holon` file's holons, into each flow that
uses it. The language has no modules (LANGUAGE.md §0, §2), and shared nodes need something to
share them through. [Components](components-L4.md) are editor panes with no permissions, not shared
code. The [n8n note](n8n-import-L4.md)'s fork 1 cited them loosely, and is corrected with this
note. The design that fits is the [registry](P3.1.1-registry-L4.md): a unit published by hash,
closure-complete, permanent.

**An imported node as a published unit:** content-addressed, with its manifest **derived** from
it, and a provenance record beside it:

| Provenance | Example |
|---|---|
| Source and version | `npm:n8n-nodes-dexpaprika@0.5.2`, the file and its hash |
| License and attribution | MIT, CoinPaprika |
| Translator and its version | the importer's version, or the agent and its prompt's hash |
| Tier per piece | 13 operations tier 1, 1 query builder tier 3 |
| Harness result | inputs tried, requests compared, fixtures' hash |

**Anyone can check offline what an integration can reach.** The manifest is computed from the
unit, never read from the provenance. The provenance is what its publisher claims. Who vouches for
those claims is [identity](P3.7.1-identity-L4.md) and [reputation](P3.4.1-reputation-L4.md)'s
question (fork 9).

How a flow uses a published node is fork 6: copying its text, which works today, or naming it by
hash, which the language doesn't have.

## 8. The order of work

The order proposed, weighed against what the survey and §7 of host capabilities show:

1. **Cheap vocabulary** (§3): `url_encode` and the query string, list `sort` and `filter`, inputs
   with defaults, the bounded pagination. Each is a leaf or a lowering; S each. No change.
2. **The pipeline, with the n8n-declarative and OpenAPI readers** (§5). Kept: both are
   declarative and need no engine. Tiers 1–3 of [n8n import](n8n-import-L4.md) §4, with
   `dexpaprika` as the first fixture.
3. **The harness** (§6). Kept. Without it, step 4 has no check.
4. **Agent translation by reading,** moved up from 5. Tier 4's 75 packages (62% of top-150
   downloads) make their requests through n8n's helpers only. An agent reading their code can write tier 1–3
   text with Holon as it is, held to the harness. It is blocked only where a door is missing:
   bytes, OAuth2, a host from a parameter. So it starts here, and widens as those land.
5. **The doors translation needs most,** moved down from 4. Bytes is already step 1.1 and first
   in Tier 1. Add **OAuth2** (§2.2) and **calls at deploy and undeploy** (§2.3) as Tier 1 steps.
   The rest of Tier 1 keeps host capabilities' own order, since a job, not translation, sets it.
6. **Engines that ask for effects** (§4): shims, snapshots, Python, **measured against step 4
   first**. If agents' translations pass the harness for most helpers-only nodes, interpretation
   covers mostly what's left: obfuscated code, very large nodes, SDK-heavy ones. Build it then
   only if that remainder is worth it.
7. **More readers,** cheapest first:
   - **The declarative ones,** right after step 2 proves the pipeline: Airbyte manifests, MCP
     tool schemas as interfaces, and, for an app's own developer, Make apps and Zapier's request
     shorthand.
   - **The code-first ones,** after step 4, since an agent does most of their work:
     Activepieces first (MIT, public), then Node-RED.
   - **Pipedream's components:** only as a user's own (§10).

**Hosts from a parameter** (23 of 72 helpers-only packages) block both step 4 and step 6, and no
door fixes them. Fork 8.

## 9. Forks, surfaced

1. **OAuth2: its own Tier 1 step, or part of email (1.3).** *Lean:* its own step. It serves every
   API, and email then uses it.
2. **A registration that fails at deploy.** Refuse the deploy, or deploy and say the trigger isn't
   registered. *Lean:* deploy, and say so where the deployment is shown, as a failed run is shown.
   A webhook service being down shouldn't block a fix to everything else.
3. **The server's public URL.** An operator's setting, or learned from the first request through
   the ingress. *Lean:* a setting. Learning it trusts a `Host` header.
4. **Retry: a param of the request leaf, or a flow-level policy.** *Lean:* the leaf's param, so it
   is in the text beside the request it governs.
5. **What runs the original node in the harness.** n8n's own runtime is under the Sustainable Use
   License: "only for your own internal business purposes or for non-commercial or personal use"
   ([LICENSE](https://raw.githubusercontent.com/n8n-io/n8n/master/LICENSE.md)). Whether an import
   service run for others is that is **unconfirmed**: a question for counsel, not this note. The alternative is a minimal
   `this` of Holon's own under Node.js, which needs no n8n code but is less independent of the
   shims it checks. *Lean:* Holon's own `this` for community nodes. It is evidence either way.
6. **Using a published node: copy its text, or `use` it by hash.** *Lean:* copy, with its
   provenance in the header comment, until sharing is real. `use` is a language change (§2 of
   LANGUAGE refuses modules) and wants its own L4.
7. **Python: RustPython or CPython-WASI with its imports stubbed.** *Lean:* neither until step 6's
   measure says an engine is worth it.
8. **Hosts from a parameter.** Asked for at import, one deployment per host (what tier 1 does), or
   bound by an admin like a folder (`host api` in the text, its address in a binding the grant
   shows). *Lean:* asked at import first. Binding is a host feature that wants a design of its own.
9. **Who vouches for provenance.** The publisher's key (identity) and reputation, or a harness run
   the registry repeats. *Lean:* the publisher signs; a re-run is a later check, not a
   prerequisite.
10. **The second reader after n8n: OpenAPI or Airbyte manifests.** Airbyte's is the most
    declarative public catalog (509 of 592 sources are manifests), but its connectors are syncs
    that pull data into a destination, and mostly ELv2. *Lean:* OpenAPI. It describes any API, not only
    extraction, and feeds tiers 1–2 directly ([n8n import](n8n-import-L4.md) §10). Airbyte comes
    next, with its syncs as clocks over collections.
11. **ELv2 and a hosted Holon.** Self-hosted Holon shipping translated Airbyte manifests with
    their notices fits ELv2. A hosted Holon offering them might be "a hosted or managed service"
    of a "substantial set" of the connector's features. **Unconfirmed:** a question for counsel.
    *Lean:* ship only the MIT ones (28 manifest-only) until it is answered.
12. **MCP tools: one host, many tools.** The approval can name only the server's host, which
    approves every tool behind it. A door for MCP (R3b) would name each tool:
    `McpTool("mcp.example.com", "list_repos")`. The host would check `tools/call` against the
    grant as it checks hosts now. *Lean:* the door, when a job uses MCP servers. Until then, MCP
    tools are imported only from servers whose every tool the approver would grant.

## 10. Refused, with the reason

- **A foreign runtime in production** (n8n, Node.js, Node-RED, a platform's worker). Permissions
  would then rest on what that runtime does, not on text a person read.
- **Shims for raw sockets, `child_process`, native addons and browsers.** No door describes them
  at a grain an approver can read. A program goes through a command an admin binds (1.7) or the
  sandbox (2.4), as host capabilities R5 says.
- **A host chosen at run time.** A permission nobody can read before the run.
- **Permissions from the importer's claims or from the harness.** The grant is derived from the
  output text, always. Provenance and harness results are shown beside it, never instead of it.
- **Obfuscated code,** even interpreted: neither its hosts nor its requirements can be shown to a
  person.
- **Shipping translations of catalogs whose license excludes it:**
  - n8n's own nodes (`packages/nodes-base`, Sustainable Use License);
  - Pipedream's components (Source Available License 1.0, which excludes commercial use);
  - any catalog that isn't public, as Zapier's and Make's apps aren't.

  A reader may still serve a user translating their own nodes. Counting a catalog, as §2.2 does,
  isn't translating it.
- **AI sub-nodes as live objects passed between nodes.** Holon has no value that is a callable
  object. What one does is a model call, and translates as one.

## 11. What stays true

- A translated node is an ordinary flow: fmt-canonical text, reviewed, approved, replayed,
  verified.
- Its permissions are derived from that text, never from the importer, the agent, the provenance
  or the harness.
- Every effect goes through a door the approval names. An engine asks; the host checks, performs
  and records.
- An engine is a pure function of its inputs and the answers so far.
- Foreign code runs only in the harness, sandboxed, at import or in CI.
- A translation keeps its source's license and attribution.
