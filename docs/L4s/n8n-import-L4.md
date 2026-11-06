# Importing n8n community nodes: translate what is provably the same, interpret the rest (L4)

**Status:** survey and design, 2026-11-06, against `b8048e9`. Nothing is built except one
translation done by hand, the sample `dexpaprika` (§11). No importer or wrapper exists. The
survey's scripts read the packages as text and are kept outside the repository; §1 says how
they worked and what that costs in accuracy.

The question: how much of n8n's community node catalog can become Holon flows without losing
what Holon promises? Those promises are three. A person reads what a flow may do before it runs.
Every run replays from its log. `holon verify` checks a run offline. The catalog is large:
14,227 npm packages carry the keyword `n8n-community-node-package`. Holon's speed matters too:
[performance](../engineering/themes/performance.md) measured Holon at about 55k requests a second,
Node-RED at about 7k and n8n at 32 to 59.

The short answer, from the 150 most-downloaded packages:

> **About one package in six is declarative.** It describes its requests instead of making them,
> and translates to ordinary Holon text: `http.*` leaves with literal hosts and `secret.*`
> headers. Most of those still carry a little JavaScript that only computes, which becomes a
> pure `js` holon beside the native parts. **Most of the rest (62% of downloads) is code whose
> every request goes through n8n's `this.helpers`.** That code can't be translated mechanically.
> It could be *interpreted* through Holon's doors, in rounds, but only after the `js` engine
> learns to pause on a request. **About one in ten does I/O of its own** (an SDK, `fs`, `net`,
> a child process) and is refused.

So the design is one pipeline applied piece by piece (an operation, an expression, a hook, a code
path), not node by node: each piece goes to the cheapest tier that provably keeps its meaning
(§4).

---

## 1. The survey, and how far to trust it

- **What was read.** npm's search for the keyword, ranked by popularity, gave 1,000 packages.
  The 150 with the most monthly downloads were fetched and 148 arrived. A random 60 from ranks
  151 to 1,000 (seed 7) show the long tail. Five keyless-API packages were fetched for §11.
- **How.** Each tarball was fetched from the registry into a new, empty folder and extracted by
  Python's `tarfile` with the `data` filter. Nothing was installed, built or run. A text analyzer
  read each node file listed in a package's `package.json` `n8n.nodes`, the local files it
  `require`s, and its credentials (`n8n.credentials`). It records:
  - the kind of node: declarative (`requestDefaults` with `routing`, and no `execute`) or
    programmatic (`execute`, `supplyData`, `poll`, `webhook`);
  - the routing features each node uses;
  - each expression in a routing block, classed as a reference (`$parameter.x`, `$value`,
    `$credentials.x`), light (references with literals, `||`, `??`, `+`), or JavaScript;
  - each credential's `authenticate` and `test`;
  - where `baseURL` and hosts come from;
  - each requirement, sorted into Node built-ins that reach outside, npm packages with I/O of
    their own, and pure ones;
  - reads of the clock, randomness and timers;
  - request call sites.
- **Accuracy.** These are regular expressions over compiled JavaScript, not a parser. They were
  spot-checked by hand: every node it couldn't class, and samples of the clock reads. The
  misses fell into two groups. Obfuscated or bundled nodes count as programmatic and unreadable.
  Two nodes build their routing from an OpenAPI spec when they load (§10). Operation counts are
  the roughest figures. A programmatic operation is an option carrying an `action:`, which n8n's
  lint asks of every operation. A declarative operation is a routing block with its own request.
- **Skew.** Two packages dominate node and operation counts. `@wiro-ai/n8n-nodes-wiroai` ships
  291 nodes, and a test package (`n8n-nodes-test-node`) has 1,095 generated operations. So the
  **package** is the main unit here, weighted by downloads where that says more. Downloads count
  installs by n8n instances, not use.
- **Licenses.** In the top 150, 147 packages are MIT and one says "see LICENSE". In the random
  60: 56 MIT, 2 Apache-2.0, 1 GPL-2.0, 1 "see LICENSE". A license rarely stands in the way. A
  translation is a derived work, so it keeps the license and the attribution in its header
  comment (as `dexpaprika` does). n8n's own nodes (`packages/nodes-base`, Sustainable Use
  License) are out of scope: never bundled, never translated and shipped.

## 2. What an n8n node is

- **Declarative.** The node's `description` holds `requestDefaults` (`baseURL`, headers) and, on
  its `operation` options and parameters, `routing` objects:
  - `request`: method, url, qs, body, and a `baseURL` of its own;
  - `send`: put this parameter in the query or body under `property`, with `preSend` functions;
  - `output.postReceive`: `rootProperty`, `set`, `filter`, `limit`, `setKeyValue`, `sort`,
    `binaryData`, or functions;
  - `operations.pagination`: `offset` or `generic`.

  n8n's routing node builds the requests from these at run time, evaluating `={{ … }}`
  expressions in its JavaScript expression language.
- **Programmatic.** `async execute()` reads its parameters (`this.getNodeParameter`), loops over
  its input items, and calls `this.helpers.httpRequest`,
  `httpRequestWithAuthentication`, `requestWithAuthentication` or the legacy `request`. Or it
  calls an SDK, or anything Node offers. The requests it makes exist only when it runs.
- **Triggers.** A webhook node (`webhookMethods` registers it with the service, `webhook()`
  answers) or a polling node (`poll()`).
- **Credentials.** Fields (`password: true` for a secret), and `authenticate`:
  - generic: `headers`, `qs`, `body` or `auth` (basic) filled from `$credentials`;
  - an `async` function, or none (OAuth2 extends `oAuth2Api`).

  An optional `test` request checks the credential.

## 3. The numbers

**Packages** (each package once, by the kind of its main nodes; trigger-only nodes left aside):

| Kind | Top 150 (148) | By downloads | Random 60 |
|---|---|---|---|
| Declarative, no functions | 9 | 2.0% | 6 |
| Declarative with `preSend`/`postReceive` functions | 15 (+1 mixed) | 17.2% | 5 |
| Routing generated from an OpenAPI spec | 2 | 3.6% | 0 |
| Programmatic, requests through `this.helpers` | 76 | 59.2% | 33 |
| Programmatic, other I/O, or unreadable | 45 | 17.9% | 16 |

**The 25 declarative nodes in the top 150:**

| Feature | Nodes |
|---|---|
| `baseURL` a literal / from the credential / from code or an expression | 11 / 13 / 6 |
| Query parameters / a body | 16 / 18 |
| A declarative `postReceive` (`setKeyValue` 7, `rootProperty` 4, `set` 2, `filter` 2, `limit` 1, `sort` 1, `binaryData` 1) | 8 |
| `preSend` functions / `postReceive` functions | 13 / 10 |
| Pagination | 3 |
| Binary data (audio, files) | 6 |
| Design-time option lists (`loadOptions`, `listSearch`) | 11 |
| At least one JavaScript expression in routing / none | 19 / 6 |

Expressions in routing blocks:

- **Classes:** 3,604 references (82%), 155 light (4%), 619 JavaScript (14%).
- **What the JavaScript does,** from a sample of 177:
  - `JSON.parse` or `JSON.stringify` of a parameter (about a third);
  - ternaries choosing a value, or `undefined` to leave a field out;
  - array and string methods with arrow functions (`split(',').map(s => s.trim())`);
  - `new Date(x).toISOString()`;
  - `encodeURIComponent` (6).

**Credentials** (168 types in the top 150):

| `authenticate` | Count |
|---|---|
| Generic header(s) | 89 |
| Generic query, header and query, header and body, basic `auth`, or empty | 16 |
| A function | 3 |
| None: OAuth2 (18), or read by the node's own code | 60 |

100 of them have a `test` request. In the declarative packages, 23 of 25 credentials put one
secret in a header or the query, and none uses a function.

**Programmatic packages, for the interpreted way** (top 150; random 60 in brackets):

| | Packages |
|---|---|
| Every request through `this.helpers`, no I/O of its own | 72 (31) |
| I/O of its own: `fetch` 18, `fs` 18, `child_process` 16, `readline` 12, `net`/`tls`/`http(s)` 16, LangChain 6, Puppeteer, MongoDB, Redis, Firebird, SSH, the MCP SDK, Tesseract (nodes) | 31 (14) |
| Computes only (text, documents, phone numbers) | 10 (3) |

The 72 that use only `this.helpers`:

| Question | Packages |
|---|---|
| Hosts: literal only / read from a credential / from a parameter | 21 / 40 / 23 |
| Reads the clock (`Date.now()`, `new Date()`): nonces, deadlines of loops that poll a job, timestamps, file names | 24 |
| Randomness (`Math.random`, `randomUUID`, `uuid.v4`) | 7 |
| Sleeps or timers | 4 |
| Paginates (a loop over pages or cursors) | 18 |
| Binary data | 30 |
| Runtime `dependencies` beyond `n8n-workflow` (all pure: `marked`, `turndown`, `zod`, `cheerio`, `form-data`, …; 4 list `axios` but don't call it) | 10 |
| Request call sites in the largest node: under 10 / 10 or more | 43 / 29 |
| JavaScript in the largest node and the local files it requires | median 81 KB (p25 32, p75 191, max 1.4 MB) |

**Coverage per tier** (§4). The highest tier any piece of a package needs:

| Tier | Top 150 packages | By downloads | Random 60 |
|---|---|---|---|
| 1–2: native Holon | 6 | 1.0% | 4 |
| 3: native plus pure `js` holons | 25 | 22.2% | 10 |
| from an OpenAPI spec (§10) | 2 | 3.6% | 0 |
| 4: interpreted through Holon's doors | 75 | 62.3% | 31 |
| 5: refused | 36 | 9.6% | 14 |
| trigger nodes only | 4 | 1.2% | 1 |

Tier 3 counts 15 declarative packages and 10 that only compute. Tier 4 counts 71 programmatic
packages and 4 declarative ones whose hook functions call out themselves.

Operations, without the two skewing packages:

| Tier | Top 150 | Random 60 |
|---|---|---|
| 1–2 | 939 | 34 |
| 3 | 196 | 138 |
| 4 | 3,383 | 661 |
| 5 | 1,256 | 147 |
| trigger nodes | 41 | 24 |

Of the 939 operations in tiers 1–2, 854 come from four packages that wrap a search, workspace
or messaging API one endpoint per operation (scavio 511, workbuddy 209, ryze 84, evolution-go
50). These counts are rough (§1).

## 4. One pipeline, five tiers, piece by piece

| Tier | A piece that is | Becomes | Its approval says |
|---|---|---|---|
| 1 | a declarative operation: base URL, method, path, query and body fields, auth | a holon calling `http.<method>("host", path, headers:, body:)` with `secret.*` | the hosts, and where each secret is sent ([secret destinations](secret-destinations-L4.md)), exactly as for a hand-written flow |
| 2 | a `={{ … }}` expression that is a reference, a default or a concatenation | a Holon expression: an input, `??`, interpolation | nothing more |
| 3 | JavaScript that only computes: a `preSend` or `postReceive` function, a complex expression, a node that only computes | a pure `js` holon beside the native parts | "run code it carries … reaching nothing outside", with its fuel |
| 4 | code that makes requests through `this.helpers` | interpreted: the code runs in the `js` engine and its requests go through Holon's doors, in rounds (§7) | "runs code it carries; may call <hosts> with <secrets>" |
| 5 | code with I/O of its own: an SDK, `net`, `fs`, `child_process`, native addons; or code that can't be read | refused, with the reason | — |

The rules:

- **Translate only what is provably the same.** A pattern the translator knows (`JSON.parse($value)`
  is `parse_json(x)`; `a ? b : undefined` is `b when a`) moves down a tier. Anything it isn't
  sure of stays in the tier above. A wrong translation is worse than a slow interpretation: it
  would show a person the wrong request.
- **Interpretation is the reference semantics; translation is an optimization.** At import,
  where tier 4 exists, the translated and the interpreted forms run on the same inputs against a
  stand-in server, and must send the same requests and give the same outputs. This is a
  differential check, as `holon verify` re-runs a run. Until tier 4 exists, the check is against
  n8n's own behaviour written down by hand, as §11 does.
- **The output is always Holon text,** fmt-canonical, with interpreted parts visibly `js` holons.
  It is reviewed, deployed and approved like any flow. When the translator learns a pattern,
  re-importing moves pieces down to native. The hash changes. With the same hosts and secrets,
  there is nothing new to approve.
- **Order of work.** Tiers 1 to 3 work with Holon as it is (§11 proves it on one node). Tier 4
  needs the `js` engine to signal a pending request and take answers in, which it can't do
  today (§7). So translation ships first, and interpretation widens coverage later.

## 5. n8n concept to Holon construct

| n8n | Holon | Notes |
|---|---|---|
| A node package | a `.holon` file, its holons copied into each flow that uses it; shared by hash once a [registry](P3.1.1-registry-L4.md) holds them ([translation at scale](translation-at-scale-L4.md) §7) | the license and attribution in the header comment |
| A resource and operation | a `holon` per operation, `holon token(network, contractAddress) -> answer` | the parameters an operation shows (`displayOptions`) are its inputs |
| A parameter and its `default` | a holon input; the default as `?? value` or a defaults record | `options` lists become documentation; a check can be a `when` |
| `requestDefaults.baseURL` (a literal) | the host literal of every `http.*` in the file, its path prefix (`/v1`) put before each path | the host **is** the permission ([outbound requests](../engineering/capabilities/outbound-requests.md)) |
| `baseURL` from the credential (`={{$credentials.url}}`, self-hosted) | the importer **asks for the host** and writes it as the literal | a host from a wire is a compile error by design; one deployment per instance |
| A per-operation `request.baseURL` | another host literal, asked for apart | `dexpaprika`'s `api-pro.dexpaprika.com` |
| `request.method`, `url` | `http.get` / `post` / `put` / `patch` / `delete` / `head`, the path as an interpolated text | `HEAD` and the five others are all there |
| `send: { type: 'query' }`, `request.qs` | the query string in the path, escaped | no leaf escapes a URL: a `js` holon does it today (§9) |
| `send: { type: 'body' }`, `request.body` | a record literal, `body: { … }` (sent as JSON) or `json(…)` | a `property` with dots becomes nested records |
| Credential fields | `secret` declarations; `optional secret` when the credential is optional | a field the credential only uses to build a host is asked for at import, not a secret |
| `authenticate`, generic `headers` / `qs` | `headers: { authorization: "Bearer {secret.api_key}" }`, or `?api_key={secret.key}` in the path | the approval says "send it to https://host" |
| `authenticate`, generic `auth` (basic) | `authorization: "Basic {base64("{secret.user}:{secret.password}")}"` | `base64` is a leaf |
| `authenticate` as a function | tier 3 when it only computes (a signature: `hmac_sha256`, `sha256` are leaves); tier 4 when it calls out | 3 of 168 |
| The credential `test` | a route or action the importer adds, `GET /_test` calling the test request | so a person can check the key after setting it |
| OAuth2 credentials | client credentials translate (sample `client-credentials`). The authorization-code flow and refresh need the host ([host capabilities](host-capabilities-L4.md) §8, fork 2) | 18 of 168 |
| `={{$parameter.x}}`, `$value`, `$credentials.x` | the holon's input, the parameter's own value, `secret.x` | tier 2 |
| `={{ … }}` in JavaScript | a known pattern in Holon, else a pure `js` holon | tier 3 |
| `postReceive: rootProperty` | a field selection, `parse_json(got.body).data` | |
| `postReceive: set`, `setKeyValue` | a record built from the answer | |
| `postReceive: filter`, `limit`, `sort` | `fold` or `map` with `when`; or a `js` holon | no list filter or sort leaf |
| `postReceive: binaryData` | not yet (§9) | |
| `preSend` / `postReceive` functions | tier 3 if they only compute, tier 4 if they call `this.helpers` | |
| `operations.pagination` | below | |
| `loadOptions`, `listSearch` | dropped | they fill the n8n editor's lists while designing, not a run |
| Input items (`getInputData()`, one run per item) | one run per call | a holon placed in `map` over a list does it per item, a request each, in one round |
| `continueOnFail`, thrown errors | an answer record with its status; absence and `missing:` | errors are values in Holon |
| A webhook trigger | a `route` | registering the webhook with the service has no place yet (§9) |
| A polling trigger | `every 5m { … }` with state for the last item seen | |

**Pagination** is the one routing feature with a loop:

- `offset` pagination adds `limit` and `offset` until a page comes back short.
- `generic` pagination asks again with `$response` in the next request until a `continue`
  expression is false.

Holon has no loop, but a bounded one is a `fold` over `range(max_pages)` whose step asks for the
next page only when the last one said there is more. The choice is lazy, so a page not needed is
never asked for (LANGUAGE.md §6.4). Each page is a round. A run's requests are capped by
`max_requests` (50, ceiling 1,000) and its time by `run_budget` (25 s), so pagination gets a
written maximum, the approval shows it as a budget, and a longer listing is the caller's to
continue with a cursor, as DexPaprika's indicators already do (`cursor`). This is the design. No
sample paginates yet, and 3 of the 25 declarative nodes need it.

## 6. Tier 3: JavaScript that only computes

A `preSend` or `postReceive` function gets n8n's `this` (`getNodeParameter`, `getNode`,
`helpers`) and the request or the items. Those that only reshape, validate or compute are
rewritten as a pure function of their inputs: the parameters they read become inputs, and the
request options or items become a record. That makes them a `js` holon (LANGUAGE.md §5.8a):
fresh realm, no clock, no randomness, no I/O. A thrown `NodeOperationError` becomes an absent
output whose reason the run records, which is what `respond …, missing:` answers. In
`dexpaprika`, `assertPathParameter` is simple enough to be Holon text (the holon `segment`), and
axios' query encoding is the one `js` holon.

**What it costs:** 6 ms a call for a trivial body, warm, and 20 ms for an edit distance
([js node](../engineering/capabilities/js-node.md) §4), against microseconds for a leaf. A route
with one such call answers in tens of milliseconds instead of about one. That is acceptable for
a call that waits on a remote API anyway, and a reason to move pieces down to native as the
translator learns patterns.

**Mechanical or by reading.** The function is TypeScript compiled to JavaScript, against n8n's
API. A tool can lift one that reads `this.getNodeParameter('x')` and returns a value. One that
walks n8n's objects needs reading, by a person or an agent, and its translation is held to the
differential check (§4).

## 7. Tier 4: running the code, every effect through Holon's doors

**The idea.** A programmatic node stays code, but every effect it has goes through a door Holon
owns:

- The compiled `dist/*.js` runs in the pure `js` engine, with `this` provided over Holon's
  leaves:
  - `getNodeParameter` reads the holon's inputs;
  - `getCredentials` reads `secret.*`;
  - `getInputData` gives the one item;
  - `helpers.httpRequest`, `httpRequestWithAuthentication`, `requestWithAuthentication` and the
    legacy `request` become a request the host performs;
  - the binary helpers wait on bytes (§9).
- **Requests by replay, in rounds.** Each round re-runs the code from the start with the answers
  recorded so far. The first unanswered request ends the call with "pending", and the host
  performs it between rounds as an `http` call that was granted and is recorded. This is how
  every request in Holon already works ([outbound requests](../engineering/capabilities/outbound-requests.md)
  §2), so `holon verify` replays the code against the log as it replays a flow.
- **Declared hosts, enforced.** The hosts are declared at import (from literals in the code, or
  asked for) and enforced at perform time, as now: a request to any other host answers
  `blocked_host`. The secrets go only where the approval shows (`blocked_secret`). The approval
  says "runs code it carries; may call <hosts> with <secrets>".

**What it keeps:**

- The approval names every host and secret.
- Every request is recorded, and a run replays offline.
- Code can't reach anything outside its doors: the engine has no imports.
- Nothing about n8n's runtime is needed.

**What it gives up:**

- *Speed.* Each round re-runs the node's JavaScript from the start, in a fresh realm. A trivial
  body costs 6 ms. A node's code is a median 81 KB (p75 191 KB) that must be parsed and set up
  on every call. Parsing that much in Boa wasn't measured. Ten to a hundred milliseconds a round
  is a guess, times the rounds. A native leaf costs microseconds. Holon's 55k requests a second
  is for native flows only.
- *Rounds.* An operation usually makes one request, so one round, and a second to read the
  answer. A paginating one (18 of 72 packages) takes one per page. A job that polls until done
  (start, then check) takes one per check, and its sleeps have no meaning between rounds: they
  become either the run's budget, or a durable `await` with a clock
  ([durable runs](../engineering/capabilities/durable-runs.md)).
- *Reading.* The requests aren't in the text. The approval names hosts and secrets but can't
  show each request's shape, as a translated operation's text does. The log shows what was sent.
- *Determinism by stubbing.* The engine has no clock or randomness. `Date.now()` (24 of 72
  packages) must be `now` wired in, the run's time, which `verify` replays. `Math.random` and
  `uuid` (7 of 72) must be the run's `random`, wired in. A nonce built from the clock, as
  `@wiro-ai`'s signing does, then repeats within a run.

**What the `js` engine would need,** none of which it has today
([js node](../engineering/capabilities/js-node.md) §4: synchronous and closed, no `async`):

1. A pending-request signal out of a call: the request record, as an `http_request` node's.
2. The answers so far passed in, keyed as the round loop keys them (node path and request hash).
3. `Promise` and `async`/`await` settled within one call: microtasks run to the end, an awaited
   unanswered request ends the call as "pending".
4. A small `this` shim over those, and `require` limited to the package's own files and pure
   dependencies bundled in (10 of 72 packages have any). An unknown `require` is tier 5.
5. A larger fuel budget for a whole node's code, measured first.

**Hosts.** 21 of the 72 packages call literal hosts only. 40 read a host from the credential,
which the importer asks for as in tier 1. 23 take it from a parameter: a URL the workflow's
author types. Those can't be one literal. Either the importer asks for the host the deployment
will use, or the operation is refused. A host chosen at run time would be a permission nobody
can read before the run, which Holon refuses by design.

## 8. Tier 5: refused, with the reason

Code with I/O of its own can't be reduced to Holon's doors:

- a socket (`net`, `tls`, `http`/`https` directly);
- a file system (`fs`);
- a process (`child_process`, `readline` over one);
- an SDK that opens its own connections (LangChain, the MCP SDK, MongoDB, Redis, Firebird, SSH,
  Puppeteer);
- `fetch` called directly.

That is 31 packages in the top 150 (9.6% of downloads, with the obfuscated ones). The importer
says which requirement and why. Most of these are Node-RED's kind of value (protocols, devices,
browsers) and need host capabilities of their own
([host capabilities](host-capabilities-L4.md) §5), not an importer. Obfuscated code (five packages
here) is refused because it can't be read: neither its hosts nor its requirements can be shown
to a person.

## 9. What doesn't translate yet, and why

- **URL escaping.** Holon has no leaf that percent-encodes, and `http.*` sends no request whose
  path has whitespace. Every `send: query` relies on axios escaping its values, and 6 routing
  expressions call `encodeURIComponent`. Today a `js` holon does it (`dexpaprika`'s `query`). A
  pure `url_encode` leaf would move this to tier 1, and is the first leaf this work asks for.
- **Bytes.** A response body is text, with invalid UTF-8 replaced, so a binary answer is mangled.
  That rules out 6 of 25 declarative nodes (speech, files) and 30 of 72 programmatic packages in
  part. The `Bytes` value ([host capabilities](host-capabilities-L4.md) §5.10) is what they wait on.
- **`multipart/form-data` uploads,** which `preSend` functions build for file operations:
  `http_upload` sends one file as the body, not a form (host capabilities §5.13).
- **A host from a parameter** (§7). A host from the credential is asked for at import: one
  deployment per instance of a self-hosted service.
- **OAuth2's authorization code and refresh** (18 of 168 credentials): a person's consent in a
  browser and a stored refresh token, which the host would have to do (host capabilities §8,
  fork 2).
- **Registering webhooks** (`webhookMethods`: create and delete a subscription with the service
  when a workflow activates). The route is easy. Calling the service at deploy and at undeploy
  has no place yet.
- **List filter and sort** have no leaves. `fold` spells a filter, and sorting is a `js` holon.

## 10. OpenAPI specs

Could a spec feed the same translation? Yes, into tiers 1 and 2. A spec states what a declarative
node states:

| OpenAPI | Holon |
|---|---|
| `servers[].url` | the host literal and a path prefix (variables asked for) |
| `paths` × method, `operationId` | a holon per operation |
| `parameters` in path, query, header | inputs, interpolated into the path, query or headers |
| `requestBody` (JSON) | `body:` a record |
| `securitySchemes`: `apiKey` (header or query), `http` bearer or basic | `secret` declarations and headers |
| `securitySchemes`: `oauth2` client credentials | the token exchange (sample `client-credentials`) |

Two of the top 150 already do this inside n8n: `@devlikeapro`'s chatwoot and WAHA nodes build
their routing from a spec when they load (`@devlikeapro/n8n-openapi-node`), 3.6% of downloads. A
spec has no `postReceive` picks, no defaults chosen for a workflow, and no pagination, so its
operations land in tier 1 with nothing to pick. It is also the cleaner source: a spec is the
vendor's statement of the API, under its own license, with no JavaScript at all. An importer
should read both, and prefer the spec for the request and the node for its defaults and picks.

## 11. The proof: `dexpaprika`, by hand

`n8n-nodes-dexpaprika` 0.5.2 (CoinPaprika, MIT) is a declarative node for DEX and on-chain market
data, keyless on its free tier. It is small (587 lines), and its one function is a `preSend`
check. It was translated whole into `crates/holon-author/samples/dexpaprika.holon`:

- 13 operations, 13 holons, each with a route to try it;
- `requestDefaults.baseURL` becomes `api.dexpaprika.com`. Two token operations set their own
  `baseURL`, `api-pro.dexpaprika.com` (paid plans), the second host the approval asks for;
- the optional credential becomes `optional secret dexpaprika_key`, sent as the whole
  `authorization` header when set (`{ …, authorization: secret.dexpaprika_key } ?? { … }`: a
  record with an absent field is absent). The approval says it is sent to those two hosts and
  nowhere else;
- `={{$parameter.network}}` in a URL becomes the holon's input, interpolated into the path;
- `send: { type: 'query' }` with the node's defaults becomes a defaults record, and a `js` holon
  (`query`) escapes the query string as axios does, leaving out empty values;
- `assertPathParameter` becomes the holon `segment`, in Holon text. An id holding `/ \ ? # %`
  (or `.`, `..`, nothing) is absent, the request isn't sent, and the route answers 400.
  Whitespace and control characters were already refused by `http.*` itself;
- each answer is the API's status and its JSON, or its text when it isn't JSON (the pro host's
  403 without a key is a Cloudflare HTML page). A call that got no answer is 502, with why.

It was compiled against the live catalog and kept as `fmt` writes it. `samples.test.ts` builds
it and checks what it asks for, and the end-to-end tests deploy it.

**Live, 2026-11-06,** deployed with this build's `holon` (rc.33) on a scratch server:

- `holon check` and the deploy's approval list two hosts, the js node's budget, requests, and
  "read the secret "dexpaprika_key", and send it to https://api-pro.dexpaprika.com and
  https://api.dexpaprika.com";
- all eleven free operations answered 200 with DexPaprika's data (networks, DEXes, stats, search,
  top tokens, a token, batch prices, top pools, a pool, its OHLCV, its RSI), in 0.5 to 1.3 s, the
  API's own latency;
- the two pro operations answered the API's 403 without a key;
- `eth%3Fx=1`, `eth%2Fpools` and `..%25` as a network answered 400 in 4 to 30 ms, with no
  request sent;
- a search for "wrapped ether" reached the API escaped, which refused it (its own validation);
- `holon verify` on the export: an unbroken chain, and 4 runs re-run exactly. The 15 that made a
  call read the secret, set or not, and an export never holds a secret's value, so they aren't
  re-run: true of every flow that reads one.

The hand translation took the node's 587 lines to 201 lines of Holon (61 of them comments), of
which 4 are JavaScript. Mechanically, the importer would need only:

- the `routing` objects;
- the parameters' `default`s and `displayOptions` (which inputs an operation takes);
- the credential's `authenticate`;
- one pattern for the `preSend` it carries.

## 12. Slices, when this is built

1. **The leaf `url_encode`** (pure, total), with `dexpaprika` moved off its `js` holon.
2. **Tiers 1–3 for declarative nodes and OpenAPI specs:** `holon import n8n <package|dir>` and
   `holon import openapi <spec>`. The output is a `.holon` file, every host asked for or read,
   every unsure piece a `js` holon or a comment naming what was left out. Packages are read as
   text from a tarball, never installed. Tested on the 24 declarative packages of the survey: each
   must compile, and its requests against a stand-in server must match those n8n's routing would
   send, written down per package.
3. **Bytes** (host capabilities §5.10), which brings in the binary nodes.
4. **Tier 4:** the engine's pending signal, answers in, `async` settled in a call, the `this`
   shim. Then measure a real node's cost per round before choosing it over translation by
   reading.

## 13. Open forks

1. **Where an imported node lives.** One `.holon` file per node, copied into each flow that uses
   it, or a unit flows share by hash once a [registry](P3.1.1-registry-L4.md) holds them.
   ([Components](components-L4.md) are editor panes, not shared code.) Copying is what works
   today. Sharing keeps one approval per node. [Translation at scale](translation-at-scale-L4.md)
   §7 takes this further.
2. **The importer's reader of JavaScript.** Regular expressions were enough to survey, not to
   translate. A real JavaScript parser (in Rust, or the `js` engine's own Boa parser exposed as
   a pure call) is needed for tier 2 and 3 patterns, and must itself never run the code.
3. **Agents for tier 3 by reading.** An agent can rewrite a `preSend` function as a pure one
   faster than a pattern library grows. Its output is held to the differential check, and a
   person still approves the flow it lands in.
4. **Is tier 4 worth building?** It covers 62% of downloads, at the cost of speed, of requests
   that can't be read in the text, and of an engine that can pause. The alternative is to
   translate those nodes by reading their code (an agent, then the differential check against a
   stand-in), which yields tier 1–3 text at native speed. Measure tier 4's cost per round on a
   real node before choosing.

## 14. Refused, with the reason

- **Running n8n nodes in Node.js beside Holon.** Their effects would be undeclared, their runs
  not replayable, and Node's speed would become Holon's.
- **A host from a credential or parameter at run time.** The host is the permission. A person
  must read it before the run.
- **Translating n8n's own nodes.** The Sustainable Use License forbids shipping them, translated
  or not.
