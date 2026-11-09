# Host capabilities: what earns one, and what Holon already covers (L4)

**Status:** design, before any code. Written 2026-10-27, against rc.27 (1ccf376), and the triggers
design as of 2befd26. Revised 2026-10-29 against rc.29 (6bc5c68): commands and persistent
terminals (§5.7), a shell in a sandbox whose walls are the grant (§5.8), outbound streams and
long-lived connections (§5.9), a bytes value with decimals decided beside it (§5.10), R5 reworded
around boundaries, and the implementation plan in three tiers (§7, replacing the order for payment
operations), with history as step 2.5 ([history](history-L4.md)). Revised 2026-11-04 against
450e3de, and again against ff03a80 (`main` after the request path's work): static files (§5.11,
step 1.8) and inbound WebSockets (§5.12, step 1.9) designed here and put in Tier 1, inbound
WebSockets having been "later" until then; what else a web app asks for, sorted (§5.13); and a
flow as a command-line app (§5.14). Revised 2026-11-06 against af9de3a: what a trading bot
adds to feeds (§5.9, forks 25–27), log levels having come in rc.32 since §5.9 was written; and
from then on every host capability is designed here (§7, §10). Revised 2026-11-07 against
490d03e: a flow as one executable, `holon build` (§5.15, forks 32–39), resolving fork 24. It sets rules, sorts candidates,
and holds each one's design: a capability's L4 is its section of §5, grown into numbered
subsections when its step starts, as §5.11 and §5.12 were, so the theme is in one place, found
and cited by section. Those designed before then (folders, databases, channels, triggers, host
resources, secret destinations, storage, history) keep their own L4s, listed in §10. Builds on
[host resources](host-resources-L4.md) (resources the host defines, their addresses),
[named folders](named-folders-L4.md), [databases](databases-L4.md),
[channels](channels-L4.md) and [triggers](triggers-L4.md) (a flow with several handlers).

Holon's promise is three things at once: **the approver reads what a flow may do before it runs**,
**every run replays from its log**, and **`holon verify` checks a run offline**. Everything a flow
can reach has to keep all three. A business asks for email, bank files over SFTP, PGP, client
certificates, ISO 20022 payment files, queues, buckets. Some of those need new host code; most
need a leaf or only a sample. This note says which, by rules, so the answer doesn't depend on who
asks or how basic the thing sounds.

---

## 1. What is already there

**What starts a run** (the manifest's trigger; `holon-core/src/kind.rs`):

- a **route**: a request on the flow's port (`NetworkIngress`);
- a **clock**: `every 2s { … }` (`Clock`);
- a **file**: `on file inbox (f) { … }`, a file settling in or leaving a bound folder
  (`FileArrival`);
- an **action**: `action reset (input) { … }`, run by a person, an agent or another flow through
  Holon, never the network (`Action("reset")`); another flow runs it with
  `billing.charge(x)` (`RunsAction("billing/charge")`, rc.29).

**What a run reaches:**

| Reach | Capability | Answered by |
|---|---|---|
| an HTTPS API | `HttpRequest("host")` | `http.get`/`post`/…, `http_upload`, `http_stream`; the host sets `Host` and the framing headers, refuses private addresses (`PrivateAddresses` lifts it) and plain HTTP (`PlainHttp("host")` lifts it) (`outbound.rs`) |
| a folder an admin bound | `FileRead`, `FileWrite`, `FileArrival` | `folders.rs`; reads logged by hash, their texts kept in `blobs.rs` |
| a database an admin bound | `DbRead("d/t")`, `DbWrite("d/t")` | `databases.rs`, `engines.rs` (SQLite, Postgres, MySQL), `mongo.rs`; one transaction when the run finishes; claims and locks (`claims.rs`, `locks.rs`) |
| its own data | `StateRead`/`StateWrite`, `CollectionRead`/`CollectionWrite` | the deployment's state, written when the run finishes |
| subscribers | `ChannelSend`, `ChannelRead` | `channels.rs`; packs sent once the run is over |
| a secret | `SecretRead("name")` | the deployment's vault, values from `env`, `vault` or a `--dotenv` file (`secret_sources.rs`); followed through the run by `taint.rs`, cut only by a one-way leaf (`sha256`, `hmac_sha256`, `secure_equals`) or a true/false answer |
| later | `Await("e")` | durable runs: `await`, `resume_token` (`durable.rs`) |

**Pure leaves** (no permission; `docs/manual/language/leaves.md`): arithmetic and comparison,
text (`slice`, `split`, `index_of`, `uppercase`…), lists and records, `json_parse`,
`json_stringify`, `sha256`, `hmac_sha256`, `hex`, `base64`, `secure_equals`, and the
`stripe.verify` template.

**A run that runs again** (its reads changed before it committed) answers its outbound requests
from its own record (`prior`, `firing.rs`), not sending them again: whatever a run asked of the
outside is asked once per run.

**Three capabilities nothing in the language writes:** `NetworkEgress` (the `egress` leaf: the
host records it as performed and sends it nowhere; no sample uses it), `SecretAccess` and
`Spawn` (`kind.rs`).

**Numbers are 64-bit floats** (`holon-core/src/value.rs`, `Number(f64)`). Integers are exact to
2^53, so amounts in minor units (cents) are safe; `"1234.56"` parsed and summed is not.

**There is no bytes value.** A value is a number, true or false, text, a list, a record, a sum or
an effect. A folder read refuses a file that isn't UTF-8 (`folders.rs`: "isn't text (UTF-8)…
read a handle of it"), so a binary file (a PDF, a zip, a PGP-encrypted statement) can be copied
by its handle but never read.

## 2. Four ways to cover something

| Way | What it is | Manifest item | Costs |
|---|---|---|---|
| **covered** | a sample (and a manual page, if it teaches something) over what exists | what it already uses (`HttpRequest("api.slack.com")`) | a sample |
| **a leaf** | pure computation in the catalog: same inputs, same output, everywhere | none | one catalog entry, its docs, `wasm-pack test` parity (it runs in the editor too) |
| **a host capability** | a new `Capability` the host performs, logged, approved, often a resource an admin binds | a new item, at the grain consent needs | the whole list in §4; usually a new crate |
| **a server feature** | something the operator configures and no flow asks for | none | server code and the server manual |

The fourth way is there because some asks aren't things a flow does: TLS for the server's own
ports (it ends at the operator's ingress, a247cc7), telemetry export, sign-in through an identity
provider. A flow never asks for them, so they never enter a manifest.

## 3. The rules

Take the first that applies.

**R1. Computation on values the run already holds is a leaf.** Parsing and writing formats,
exact decimals, dates, compression, signatures checked with a public key. Never a capability:
it touches nothing outside. A leaf must be deterministic on every platform the catalog runs on
(native and wasm), and bounded by the run's limits (a 10 MiB zip that inflates to 10 GiB is
refused, not run).

**R2. An HTTPS API whose host tells the approver enough is covered.** "Call
https://hooks.slack.com" is what the approver needs to know about a Slack message. So Slack,
Teams, Twilio, Stripe, model APIs and most of SaaS need a sample, not code. The sample shows the
secret in a header, the signature check (`hmac_sha256`, `secure_equals`), and the limits.

**R3. A host capability is earned when at least one of these holds:**

- **a. The protocol isn't an HTTPS request and its answer.** SMTP, IMAP, SFTP, a queue's
  consumer, anything long-lived or stateful. A flow can't speak it, so the host must.
- **b. Consent needs a finer grain than a host.** If the meaningful question is *who receives
  this*, *which remote path*, *which topic* or *which bucket prefix*, then "call
  https://api.postmarkapp.com" approves too much. Such a capability may still be HTTPS
  underneath; what it adds is the item the approver reads.
- **c. The security step is the host's to hold.** Key material that must never pass through a
  run (a PGP private key, a TLS client key), or a signing scheme a flow would get subtly wrong
  (AWS SigV4). Note the taint rule (`taint.rs`): what is computed from a secret is withheld from
  the log, and a state, collection or channel write of it is refused. Decrypting with a secret
  key yields exactly that, so a run that decrypted a bank statement could neither keep the
  plaintext nor be replayed from its log. Decryption **can't** be a leaf over a secret: it has to
  be a key the host holds, whose output is ordinary data (§5.3). Signing can: a signature is
  one-way, like `hmac_sha256`.
- **d. It starts runs from outside the flow's port.** A mailbox, a remote folder, a queue: a new
  kind of handler (triggers L4), with a settle rule and a de-duplication key.

**R4. Earning a capability doesn't schedule it: the plan does (§7).** The rules say *how*
something is built; §7's tiers say *when*. (Until 2026-10-29 the job said when, by
`PLATFORM_LAYOUT.md` §7's rule that a feature no step toward M3 needs waits; for host
capabilities, the tiers replace it.)

**R5. No code outside a boundary the manifest describes.** A shell, or `Spawn` (start any
process), with the server's own reach puts anything at all behind one item: it can read the
secrets file, call any host and rewrite the server, and nothing it did is in the grant or the
log. That stays out, for flows and for agents. A program may run in one of two ways:

- **as an admin bound it** (§5.7): the program, its working folder and its environment fixed on
  the host, the flow filling typed arguments (a **command**) or text sent to it (a **terminal**),
  never a command line; the manifest describes the program;
- **inside a sandbox whose walls are the grant** (§5.8): any code, a shell included, where the
  operating system enforces that it sees only the bound folders, reaches only the granted hosts
  through Holon, holds only the secrets passed to it and stays within its limits; the manifest
  describes the walls.

Walls that leak are worse than none, because the approver believes them: where the operating
system can't enforce a wall, the sandbox isn't offered, rather than offered weaker under the
same words.

## 4. What every host capability must answer

Each one's L4 has a section for each of these. A capability that can't answer one isn't ready.

1. **The item and its words.** Name, what in the text produces it, at the grain R3b asks for:
   `MailSend("ops")` with its recipients, not `NetworkEgress`.
2. **The resource.** Bound by an admin (the host-resources model: a flow names it, the host
   decides where it is, `--for <slug>` for one deployment), or named in the text as a host is.
   Credentials come from a secret source; they are never in the text, the log or the editor.
3. **Reads are the run's input.** What the host fetched is logged (or kept by hash in
   `blobs.rs`), so a replay and `holon verify` need no network, as HTTP rounds and database
   reads do today.
4. **Effects land when the run finishes, at most once.** A sent mail or pushed file can't be
   undone, so it goes out after the run's writes land, as a channel's packs do, and a rerun
   (a database conflict, a resumed run) never sends twice. Where the remote end can't be
   transactional, the L4 says what a crash between "written" and "sent" leaves.
5. **Sharing.** If two runs can touch one remote thing, claims or locks (`claims.rs`, `locks.rs`)
   cover it, and the L4 says whether that holds across servers.
6. **Triggers.** What starts a run, how a thing is known to be complete (the settle rule), how it
   isn't run twice (the key), and what happens to it after (moved, marked, acknowledged).
7. **Secrets.** What `taint.rs` does with it: what may carry a secret-derived value out, and what
   is refused.
8. **Limits.** Sizes, counts, rates, timeouts: each with a default and a ceiling in the policy,
   shown in the deploy bar.
9. **Verify.** What `holon verify` recomputes, and what it can only check by hash.
10. **Seeing it.** The admin page's resources tab, the deploy bar, `holon check`'s words, the
    manual's permissions page and a guide.
11. **The sample** (CLAUDE.md): run live once, and deployed by the e2e tests **against a local
    stand-in** (an SMTP sink, an SFTP server, a queue) the tests start, so CI needs no outside
    account and no network.
12. **The engine.** The crate: pure Rust preferred (one build on Windows, macOS and Linux through
    cargo-dist, no system libraries), its size, its maintenance, rustls for TLS.

## 5. The candidates, sorted

| Candidate | Way | Rule | Notes |
|---|---|---|---|
| Slack, Teams, SMS (Twilio), Stripe, model APIs, GraphQL, any REST API | covered | R2 | samples; Stripe and models have them already |
| webhooks out, signed | covered | R2 | `hmac_sha256` over the body, a header |
| SOAP | covered, once XML leaves exist | R1, R2 | XML over HTTPS |
| binary data (files, attachments, PGP, sockets) | a **value kind**: `Bytes` | R1 | §5.10; before what needs it |
| CSV | leaf | R1 | §5.1 |
| XML (enough for ISO 20022) | leaf | R1 | §5.1 |
| exact decimals | leaf | R1 | §5.1 |
| dates and times | leaf | R1 | §5.1 |
| regular expressions | leaf | R1 | a linear-time engine only (the `regex` crate) |
| zip, gzip | leaf | R1 | inflated size counted against the run's limits |
| JWT, PGP: verify, encrypt to a public key, sign | leaf | R1 | signing cuts taint like `hmac_sha256` |
| PGP decrypt; later KMS and HSM | host capability: **keys** | R3c | §5.3 |
| email: send | host capability | R3a, R3b | §5.2 |
| email: receive | host capability (a trigger) | R3a, R3d | §5.2 |
| SFTP | host capability: a **folder backend** | R3a, R3c, R3d | §6 |
| object storage (S3, GCS, Azure Blob) | host capability: a folder backend | R3b, R3c | §6 |
| client certificates (mutual TLS) | host capability: an **identity** | R3c | §5.4 |
| queues (SQS, RabbitMQ, Kafka, NATS) | host capability | R3a, R3d | §5.5 |
| approvals by role, in the editor | host capability (extends `Await`) | R3b | §5.6 |
| telemetry (OpenTelemetry), audit to a SIEM | server feature | — | OTLP over HTTP, so no gRPC stack |
| TLS on the server's ports | server feature, at the ingress | — | decided: a247cc7 |
| Redis, a key-value cache | not needed | — | state and collections; a database if shared |
| gRPC | later | R3a | only when a job brings one; HTTP/2 and protobuf are a large engine |
| PDF output | later | R1 | a leaf, deterministic only with fonts fixed in the binary |
| a model with a budget | later | R3b | `HttpRequest` covers it today; a token or cost budget as the item is the interesting part |
| a streamed HTTP answer in one run; polling | covered | R2 | `http_stream` (rc.23), `every` and `http.get` |
| an SSE, WebSocket or gRPC feed; Postgres `LISTEN`; MQTT | host capability: a **feed** (a trigger) | R3a, R3d | §5.9 |
| a WebSocket session (send, wait for the reply) | host capability: a terminal backend | R3a | §5.9 |
| a file as a route's answer, any size, binary too | host capability: `FileServe` on a folder | R3b | §5.11, step 1.8; answering callers with a folder's files is a finer item than reading them |
| a route for the rest of a path (`/*path`) | not a capability: routing | — | §5.11, step 1.8 |
| binary bodies a browser sends; big uploads; `multipart/form-data` | the value kind (`req.bytes`); big ones staged, as a handle (`req.file`); a leaf for multipart | R1 | §5.13 |
| several cookies in one answer; sessions | covered, once a header's value may be a list | R1, R2 | §5.13; signed with `hmac_sha256`, or an id in a collection |
| a Content-Security-Policy | covered (a header); an operator's default later | — | §5.13; a server feature, never derived |
| a flow's own files, bundled with its version | later: a unit feature, no item | — | §5.13 |
| React rendered on the server | later: a library module for `js` holons | R1 | §5.13; no streaming render, no fetching in a render |
| a flow as a command-line app (`todo add "buy milk"`) | server feature: `holon do` and `holon command` over actions; actions that resume; a REPL over §5.12 | — | §5.14; no new item, and no raw terminal piped into a run |
| a flow as one executable, run with no server (`holon build`) | server feature: a runtime with one flow inside, approved at build and accepted on first run | — | §5.15; resolves fork 24; no new item |
| ISO 8583, FIX, SMPP, a bank's own socket | host capability: a **socket** | R3a, R3c, R3d | §5.9; when a job brings one |
| inbound WebSockets | host capability: a trigger on the flow's port, `WebSocket(path)` | R3a, R3d | §5.12, step 1.9 |
| a shell or `Spawn` with the server's reach | never | R5 | for flows and for agents |
| a shell in a sandbox whose walls are the grant | host capability: a **sandbox** | R3c, R5 | §5.8; Linux first, refused where the OS can't hold the walls |
| a program an admin binds (a vendor CLI, a script) | host capability: a **command** | R3c, R5 | §5.7 |
| a persistent terminal (an interactive program, an SSH session, a mainframe screen) | host capability: a **terminal** | R3a, R3d, R5 | §5.7; the last resort, after a command or an API |
| `egress` / `NetworkEgress` | retire | R3b fails | a message with no destination approves nothing (§8, fork 7) |

### 5.1 Leaves for payment files

Payment operations live in files: CSV exports, ISO 20022 XML (pain.001 to pay, pain.002 for its
status, camt.053 for statements), older bank formats (MT940, BAI2, NACHA). None needs a
permission, and every one is blocked today by four missing leaves:

- **`csv_parse(text) -> list of records`, `csv_format(records) -> text`**: RFC 4180, a header row
  or none, a separator.
- **`xml_parse`, `xml_format`**: elements, attributes and namespaces into records and back. No
  DTDs and no external entities, by construction (XXE can't happen if the parser never fetches).
- **Exact decimals.** `decimal_add`, `decimal_sub`, `decimal_mul`, `decimal_compare`,
  `decimal_round` (half-even, to a scale) and formatting, on amounts written as text
  (`"1234.56"`). A pain.001's control sum must equal its lines' sum exactly; with `f64` it may
  not. Whether an amount is text or a value of its own is decided with bytes (§5.10).
- **Dates**: parse and format ISO 8601, add days, the weekday; business days given a list of
  holidays (the list is data, not built in). Time zones need the tz database in the binary
  (§8, fork 3).

MT940, BAI2 and NACHA come after, as samples built from text leaves (or a fixed-width leaf), not as
leaves of their own: there are too many formats to put each in the catalog.

#### 5.1.1 As designed (2026-11-08, against `40cfc0a`)

Step 1.2 is six slices, each a commit with its sample, in the order a payment file needs them:

| Slice | What | Sample |
|---|---|---|
| 1.2a | exact decimals and CSV (below) | `payout-totals`: a CSV of payouts dropped in a folder, each currency's total exact |
| 1.2b | dates with a fixed offset; adding days and business days, holidays given as data | `due-dates` |
| 1.2c | XML: `xml_parse` and `xml_format` | `payment-batch` (§7): a CSV of payouts in, a pain.001 out, its control sum exact |
| 1.2d | gzip and zip, and writing bytes to a file | `statement-archive` |
| 1.2e | JWT verify: HS256 over `hmac_sha256`, RS256, ES256 and EdDSA with a public key or a JWKS | `jwt-api` |
| 1.2f | PGP: encrypt to a public key, verify a signature | `pgp-outbox` |

Decimals, CSV, dates and XML are written in core by hand: each is small, exact and fetches
nothing. Inflating, RSA, elliptic curves and OpenPGP are not, and each would be a new
dependency of core (§8, fork 28); PGP's encryption is random by design (fork 29). Those slices
are designed in detail when they are built; 1.2a is designed here.

**Decimals** (§8, fork 1: text leaves). A decimal is a text: an optional sign, digits, an optional
point and fraction, an optional exponent (`-12.30`, `.5`, `1e-8`), as `Number(…)` reads one, but
no spaces, `Infinity` or hex. A leaf takes a finite number too, as its shortest text (`0.1` is
`0.1`, not the binary value under it), so a JSON number reaches it as written up to 15 digits.
Each leaf gives nothing for anything else, as the others do.

| Leaf | Gives |
|---|---|
| `decimal_add(a, b, …)` | the exact sum; its scale the largest of theirs (`"1.10"` and `"2"` give `"3.10"`) |
| `decimal_sub(a, b)` | `a − b`, the same way |
| `decimal_mul(a, b, …)` | the exact product; its scale the sum of theirs (`"19.99"` times `"3"` is `"59.97"`) |
| `decimal_div(a, b, scale) {rounding}` | `a ÷ b` to `scale` places; nothing when `b` is zero |
| `decimal_round(a, scale) {rounding}` | `a` to `scale` places, padded with zeros: how an amount is written (`"5"` to 2 is `"5.00"`) |
| `decimal_sum(list)` | the exact sum of a list's decimals (a control sum); `"0"` for none |
| `decimal_compare(a, b)` | −1, 0 or 1 by value: `"1.10"` and `"1.1"` are equal |

`rounding` is a family: `half_even` (the default, a bank's), `half_up`, `half_down`, `up` (away
from zero), `down` (toward zero), `ceiling` and `floor`, Java's and decimal.js's seven. `scale`
is a whole number from 0 to 1,000. A result is written plainly, never with an exponent, without
`+` or a negative zero, and with a digit before the point (`"-0.50"`, `"0"`). No input or result
may have more than 1,000 digits (`DECIMAL_DIGITS`), so a product of two such is bounded, as R1
asks. JavaScript has no decimal, so every leaf is called by name; `Number(x)` is a decimal's
nearest number, as JavaScript reads the text.

**Keeping a flow from `+`** (§5.10.1's cost). A decimal leaf's result is a text, so `a + b` on two
of them joins them, as JavaScript does, and `a < b` compares them letter by letter (`"10.00" <
"9.00"`). The compiler refuses `+`, `<`, `<=`, `>` and `>=` where one side is a decimal leaf's
result (directly, or through a `const` that names one), naming the leaf to use: `decimal_add`,
`decimal_compare`. `===` stays, and compares the texts (two amounts rounded to one scale are equal
when their texts are); `-`, `*` and `/` stay JavaScript's, a number from each text. A decimal that
arrives from outside (a CSV field, a column) is a text like any other and isn't known as one.

**CSV** (RFC 4180, both ways):

| Leaf | Gives |
|---|---|
| `csv_parse(text) {header, separator}` | with `header: "first_row"` (the default) a list of records, keyed by the first row's names; with `"none"`, a list of lists. Every field is a text |
| `csv_format(rows, columns?) {header, separator}` | the text, each line ended with CRLF, the last too. Given `columns` (a list of names), each row is a record and its fields are written in that order, a missing one empty, with the names as the first line unless `header: "none"`; without, each row is a list |

`separator` is a family: `,` (the default), `;`, a tab, `|`. Parsing takes CRLF or LF, a quoted
field with `""` for a quote and the separator or a line break inside it, and drops a leading
byte-order mark (Excel writes one) and lines with nothing on them. It gives nothing for a quote
that never closes, text after a closing quote, a quote inside an unquoted field, a row with a
different count of fields than the header, or a header naming a column twice: a payment file
that is malformed is refused, not guessed at. Writing quotes a field holding the separator, a
quote or a line break; a number is written as `String(n)` writes it, a Boolean as `true` or
`false`, and anything else (a record, a list, bytes) gives nothing. A record's keys are kept in
order of their names, so `columns` is how a file's order is said. JavaScript has no CSV either:
both by name.

**The sample, `payout-totals`:** a CSV of payouts (`id,currency,amount`) dropped into a folder;
for each file, each currency's count and exact total, kept in a collection and read by a route.
Its test sums amounts whose float sum is off (`0.10` and `0.20` give `0.30`, not
`0.30000000000000004`).

#### 5.1.2 Slice 1.2a as built (2026-11-08)

As designed, with what building it found:

- **Core:** `decimals.rs` (digits as base-10 vectors, schoolbook product, long division, one
  rounding rule for `decimal_div` and `decimal_round`) and `csv.rs`; 168 primitives. A
  comparison is never refused for size: `decimal_compare` of two 1,000-digit decimals compares,
  where their difference would pass the bound.
- **The `+` guard** looks at the node that made each side (`DECIMAL_TEXTS` in `lower.rs`), so a
  `const` naming a decimal leaf's result is caught too; a decimal returned by a flow's own
  function isn't, since its result is that function's.
- **The editor's palette** groups come from `paletteGroups.ts` now, and a test holds every
  catalog leaf to one group: 1.1's bytes leaves and `file_read_bytes` were in none, so they
  weren't in the palette. They are now, beside the decimal leaves and CSV's.
- **Three gaps the sample ran into, fixed after it:** `e[0]` on a value of no known type was read
  as a record's field with a number for its name, which failed validation with a message that
  didn't say so (it named a count; `EncapError::Invalid` names each error now); a parameter's
  annotation (`amounts: string[]`) didn't make `.length` known inside the function; and the
  printer refused a map's function using values from around it that calls a function returning
  a record, as two statements: the call's outputs bound, then read back into the record. A
  number index is `get` now, an annotated port is known by its type, and a call read whole prints
  as the call. The sample uses a named function and `e[0]`. The printer still leaves a
  parameter's annotation out, as the graph doesn't hold it.

#### 5.1.3 Slice 1.2b as designed (2026-11-08, against `51dc858`)

Five date leaves exist (`dates.rs`): JavaScript's `Date` in UTC, a date its time value in
milliseconds, a leaf taking a time value or a text as `new Date(x)` does. `date_parse` already
reads an offset (`2026-11-06T16:30:00+01:00`). What a payment file still needs is the calendar at
an offset (a bank's cut-off, its local date), adding days and months, and business days over a
calendar of closing days given as data. All of it is written in `dates.rs` by hand, in `i64` on
the proleptic Gregorian calendar as the rest is, and fetches nothing.

**An offset** is `"Z"` or `"±HH:mm"` (`HH` up to 23, `mm` up to 59), as ISO 8601 and `Date.parse`
write one, or a whole number of minutes east of UTC from −1439 to 1439. Anything else, and the
leaf gives nothing. It is fixed: Frankfurt is `+01:00` in winter and `+02:00` in summer, and
which applies is the flow's data or its caller's; zones and their rules stay fork 3.

**Two leaves take an offset**, an optional input, so the forms that lower to them now leave it
unwired and mean what they meant:

| Leaf | With an offset |
|---|---|
| `date_part(time, offset?) {part}` | the part at that offset (its hours, its date, its weekday); `time` is the instant, the same at any offset |
| `date_text(time, offset?) {format}` | `iso` writes the time at the offset, with it (`2026-11-06T16:30:00.000+01:00`; `Z` for a zero offset, as without one); a new format, `date`, the date alone (`2026-11-06`, `toISOString`'s first ten); `utc` is GMT's, so nothing for an offset other than zero |

**Four new leaves:**

| Leaf | Gives |
|---|---|
| `date_add(time, amount, offset?) {unit}` | the time value `amount` days, months or years later (earlier when negative), at the offset's calendar; `unit` a family, `days` (the default), `months`, `years`. A month keeps the day of the month, or is the month's last day when it has fewer (January 31 and a month is February 28, as date-fns's `addMonths`); a year is twelve months. The time of day is kept |
| `business_day_add(date, days, holidays?) {weekend}` | the calendar date, as `YYYY-MM-DD`, `days` business days after `date` (before it when negative); with 0, `date` when it is a business day, else the next one |
| `business_days_between(from, to, holidays?) {weekend}` | how many business days come after `from` up to `to`, `to` included; negative when `to` is before `from`. So `business_days_between(d, business_day_add(d, n))` is `n` for any `n` but 0 |
| `is_business_day(date, holidays?) {weekend}` | whether `date` is one |

A business day is a day that is neither in the weekend nor a holiday. `weekend` is a family:
`sat_sun` (the default), `fri_sat`, `none`. A calendar date (`date`, `from`, `to`, a holiday) is a
`YYYY-MM-DD` text, or a time value, read as its date in UTC; a text with a time of day is
refused, since its date depends on an offset the leaf can't know: `date_text(t, offset, { format:
"date" })` says which date is meant. `holidays` is a list of calendar dates, a bank's closing days:
the list is data the flow holds or reads, not built in; an item that isn't a date gives nothing,
as a malformed CSV does. A business-day leaf gives a date as text because a date has no time and
no offset: it goes into an XML file or a holiday list as it is, and `Date.parse` reads it as the
day's start in UTC. `days` is a whole number up to 100,000 either way; `business_day_add` steps
day by day, at most `|days| × 7 / 5` steps plus the holidays and a week, and
`business_days_between` counts whole weeks and then the holidays in the range, so neither depends
on the range's length.

**JavaScript's forms.** JavaScript has no business days, no clamping month, and a `Date` that
writes no offset, so the four new leaves and the offsets are called by name; `d.setUTCMonth(m +
1)` still lowers to `date_set` and overflows as JavaScript's does (January 31 and a month is March
3). The library table (javascript-library.md §8) maps date-fns's `addDays`, `addMonths`,
`addYears`, `addBusinessDays` (no holidays there), `differenceInBusinessDays` and `isWeekend`.
The printer writes `date_part` and `date_text` as `new Date(t)`'s methods when the offset is
unwired, and by name when it is wired or the format is `date`.

**The checklist (§4)** is short, since every leaf is pure: no permission, no host, nothing logged
beyond a run's values, the same answer natively and in wasm (integers throughout), and limits in
the leaves themselves. `date_part` and `date_text` change interface, so their hashes move, with
those of the samples using them.

**The sample, `due-dates`:** a route that says when a payout received now (or at a time the
request gives, at the offset it gives) is executed and credited, against a 16:00 cut-off and
TARGET2's closing days in 2026 held as a list; and one that says when an invoice is due, some
months after it was issued, moved to the next business day.

#### 5.1.4 Slice 1.2b as built (2026-11-08)

As designed, with what building it found:

- **Core:** `dates.rs` gains the offsets, `date_add` and a `Calendar` (a weekend by weekday and
  the holidays as a set of days); 172 primitives. `business_days_between` counts the whole range
  representable at once, from −271821 to +275760.
- **The sample's calendar is a function**, `closed()`, returning the list: the top level binds
  only literals, and a list of texts isn't one yet.
- **An answer with a part missing is none:** an invoice issued `2026-01-31T10:00:00Z` (a time of
  day) gives no business-day count, so the route's record isn't built and it answers 404, as a
  bad offset does. Strict, as a malformed CSV is.
- **The printer** writes `date_part` and `date_text` as `new Date(t)`'s methods only while the
  offset is unwired; `date_set` keeps its rule (every value wired).

#### 5.1.5 Slice 1.2c as designed (2026-11-08, against `86ff439`)

A pain.001 is XML whose schema fixes the order of every element: in a group header `MsgId` comes
before `CreDtTm`, `NbOfTxs`, `CtrlSum` and `InitgPty`, and a bank refuses the file otherwise. A
record's keys are kept in order of their names, so XML can't be records the way fast-xml-parser
makes them (`{ GrpHdr: { MsgId: … } }`): writing one back would put `CreDtTm` first. Nor can one
child or several change a value's shape, fast-xml-parser's known trap (`TxInfAndSts` a record
when the bank rejected one payment, a list when it rejected two).

**An element is a list**, as JsonML writes one: its name, then a record of its attributes when it
has any, then its children in order, each an element or a text.

```ts
["InstdAmt", { Ccy: "EUR" }, "100.00"]          // <InstdAmt Ccy="EUR">100.00</InstdAmt>
["GrpHdr", ["MsgId", id], ["NbOfTxs", n]]       // <GrpHdr><MsgId>…</MsgId><NbOfTxs>3</NbOfTxs></GrpHdr>
```

The same form both ways, so a file read and written back is the same file, and one written in a
flow is a list literal with its order as written, a `.map` spreading its items into it (`[...,
...payments]`). Names are kept as written, a prefix too (`ns:Document`), and a namespace is the
`xmlns` attribute that declares it, written and read as any other (fork 31). Reading is done
through leaves, as Python's ElementTree reads one (`find`, `findall`, `findtext`, `get`), not by
indexing the list.

| Leaf | Gives |
|---|---|
| `xml_parse(text)` | the root element. Nothing for text that isn't well-formed XML 1.0, or that has a DOCTYPE |
| `xml_format(element) {declaration, layout}` | the text. `declaration` a family: `utf8` (the default) writes `<?xml version="1.0" encoding="UTF-8"?>` and a line break first, `none` nothing; `layout` a family: `compact` (the default), or `indented`, two spaces a level, an element whose children are all elements writing each on a line of its own |
| `xml_find(element, path)` | the elements along `path`, in document order: `"CstmrPmtStsRpt/OrgnlPmtInfAndSts/TxInfAndSts"`, its first step among `element`'s children, each step a name or `*`. An empty list when none matches |
| `xml_text(element, path?)` | the element's text: every text in it, at any depth, joined, as the DOM's `textContent`; with `path`, the first match's. Nothing when none matches |
| `xml_attribute(element, name)` | the attribute's value, or nothing |

**Reading** (`xml_parse`) is XML 1.0 without a DTD: a byte-order mark and the XML declaration
dropped, line ends made `\n` (XML's §2.11), the five named entities and character references
read, CDATA read as text, comments and processing instructions dropped, and a text that is only
white space (the indentation between elements) dropped; any other text is kept as written,
spaces too. It gives nothing for a DOCTYPE, so no entity is ever defined and none is fetched
(XXE and billion laughs can't happen: there is nothing to expand); for an entity other than the
five, a tag that doesn't close or closes another, an attribute given twice, a name or a
character XML doesn't allow, `]]>` in text, or anything but white space, comments and
instructions around the root. Elements nest at most 64 deep, as `json_parse`'s values do, and
an attribute's white space is read as XML reads it (a tab or a line break inside one is a
space).

**Writing** (`xml_format`) escapes `&`, `<` and `>` in a text, `&`, `<` and `"` in an attribute
(its tabs and line breaks as character references, so they read back), and writes an element
with no children as `<Name/>`. A child or an attribute that is a number is written as
`String(n)` writes it, a Boolean as `true` or `false`. It gives nothing for a name that isn't an
XML name, a character XML can't hold (a control character other than a tab or a line break), an
item after the name that is neither a record (second), an element, a text, a number nor a
Boolean, or elements nested past 64. `indented` leaves an element with any text in it on one
line, so mixed content keeps its spaces.

**JavaScript's forms.** JavaScript has no XML in the language (E4X is gone), and Node none in
its library, so every leaf is called by name. The library table (javascript-library.md §8) maps
fast-xml-parser's `XMLParser.parse` and `XMLBuilder.build` (the shape differs, as above) and the
DOM's `querySelectorAll`, `textContent` and `getAttribute`.

**The checklist (§4)** is short again: every leaf is pure, fetches nothing and needs no
permission; a number is written by `js_string`, the parity-tested writer, and nothing else
touches a float; the limits are the input's size and the depth.

**The sample, `payment-batch`:** `POST /batch` takes a CSV of payouts (`id,name,iban,amount,
reference`) and answers the pain.001.001.09 credit transfer for it, its control sum
`decimal_sum`'s and each amount rounded to cents; the execution date is the request's or the next
business day at Frankfurt's offset (1.2b), and the message id a hash of the file and that date,
so the same file sent twice for one day has one id and the bank refuses the second. A CSV that is
malformed, or an amount that isn't a decimal more than 0, answers 400 through `try`/`catch`.
`POST /status` reads the bank's pain.002 status report back: the group's status and each
payment's, with its reason code when rejected, refusing a file whose namespace isn't pain.002's.

#### 5.1.6 Slice 1.2c as built (2026-11-08)

As designed, with what building it found:

- **Core:** `xml.rs`, a reader over the text with no branch for a DTD at all (a `<!` that isn't a
  comment or CDATA is a name that isn't one) and a writer; 177 primitives. `xml_text`'s path is an
  optional input, as an offset is.
- **The language needed nothing new:** a list literal mixing a name, a record and lists, with
  `...payouts.map(transfer)` spread into it, compiled as written, and the formatter lays it out
  one child a line.
- **The sample, run live:** Python's ElementTree read its pain.001 with the schema's order, a
  batch of 1,000 payouts gave the control sum Python's `Decimal` gives, and an XXE and a billion
  laughs posted to `/status` were refused as not XML.
- **Against the ISO 20022 schemas:** pain.001.001.09.xsd and pain.002.001.10.xsd, from
  iso20022.org's message archive, run through .NET's validator. Every pain.001 the sample wrote
  was valid (three payouts, the next business day, escaped text, 1,000 payouts), and so was the
  pain.002 its test reads; three copies broken by hand were refused, so the check was real. The
  schema found what the sample let through: an id past 35 characters, a name or a reference empty
  or past 140, an IBAN not in its shape, an empty date. Those wrote a file the bank would refuse,
  so the sample now answers 400 for them, and for a control sum past the schema's 18 digits and a
  date that isn't a calendar day as written; a file at every limit validates.

#### 5.1.7 Slice 1.2d as designed (2026-11-08): zip, gzip, and bytes written to a file

Banks deliver statements as a zip of camt.053 files or one gzipped, and an archive or an auditor
takes them back zipped. Four leaves, the names of Node's `zlib` and fflate's `zipSync` and
`unzipSync` (the zip library JavaScript reaches for now):

| Leaf | Gives | JavaScript |
|---|---|---|
| `gzip(data)` | a text's UTF-8 or bytes, gzipped: deflate at level 6, no file name, time 0 | `zlib.gzipSync(data)`, fflate's `gzipSync` |
| `gunzip(bytes)` | the bytes inside, each member's joined as `gunzip` does | `zlib.gunzipSync(b)` |
| `zip(files)` | a zip of a record of files, name to a text (its UTF-8) or bytes, each deflated, in name order | fflate's `zipSync(files)` |
| `unzip(bytes)` | a record of a zip's files, name to bytes, its directories left out | fflate's `unzipSync(b)` |

**The same bytes everywhere** (R1). A gzip's header holds a time and the system that wrote it,
and a zip's entries a date: `gzip` writes time 0 and system 255 (unknown), `zip` dates every entry
1980-01-01 at midnight, the first date a zip can hold, and neither reads the clock. Deflate is
`miniz_oxide`'s (fork 28, decided for compression), pinned in `Cargo.lock` as `libm` is, since a
new version may compress differently and a logged run must give the same bytes when verify runs
it again. CRC-32, which both formats carry, is twenty lines and written by hand.

**Bounded** (R1). Core can't see the run's limits, so the leaves have their own, the same on every
platform as `MAX_TEXT` is: `gunzip` and `unzip` give nothing past 64 MiB inflated, counted as
they inflate, so a 10 MiB file that would inflate to 10 GiB stops at 64 MiB; and `unzip` nothing
past 10,000 files.

**Nothing for what isn't one**, as the other leaves: a gzip whose header, checksum or length is
wrong, or with bytes after its last member that aren't another; a zip without its end record, an
entry encrypted or compressed other than stored or deflated, a checksum or size that doesn't
match, two entries of one name, or the 64-bit form (it holds more than 64 MiB anyway). A name
without the zip's UTF-8 flag that isn't UTF-8 is read as Latin-1, as fflate reads one. `zip`
gives nothing for a name that would land outside the folder it is unzipped into (empty, starting
with `/`, holding a `..` part or a `\`), or a value that isn't a text or bytes.

**Bytes written to a file.** `file_write` and `file_create` take a text or bytes, as Node's
`fs.writeFile` does: `archive[name] = zip(files)`. Their port stays `text`, typed `Any` as
`sha256`'s is. The host writes the bytes as they are; the log keeps a write by its size and hash,
as for a text, so the bytes never enter it; the run's `max_file_write_bytes` counts them.

**The sample, `statement-archive`:** a bank's delivery dropped in a folder, a zip of camt.053
statements or one gzipped, is unzipped or gunzipped; each statement's account, date and closing
balance are kept (1.2c's XML), and the delivery is archived as one zip with a manifest (CSV, each
file's SHA-256) beside its statements, written as bytes. A route answers a statement from the
archive, gzipped when the client takes it. One write a run: a loop that writes a file per item
waits for the TypeScript slices.

#### 5.1.8 Slice 1.2d as built (2026-11-08)

As designed, with what building it found:

- **Core:** `compress.rs`, with CRC-32 by hand and `miniz_oxide`'s deflate and inflate; 181
  primitives. Inflating goes through its streaming state 64 KiB at a time, so the 64 MiB budget is
  spent as the bytes come out, never after. Python's `gzip` and `zipfile` wrote the test vectors
  (a file name and two members in a gzip, a folder entry in a zip), and a test pins the SHA-256 of
  the bytes `gzip` and `zip` write for a fixed text: a `miniz_oxide` that deflates differently
  fails it before it changes what a logged run gives.
- **The host:** a write's payload holds a text or bytes. The log keeps `{name, size, hash}` over
  the bytes, and a run's timeline leaves out a node's output that carries bytes written past the
  inline size, as it does for a text, so an archive is in the log by its hash alone.
- **The language** needed one fix, found writing the sample's `summary`: a key a function returns
  named like a binding of its body. `{ total: total * 2, again: total }` read the output in
  `again`, and `{ again: total, total }` failed with "unknown name total", since the binding had
  become the output. The binding is now renamed first, in the body and in every key's value, and a
  declared output once bound is a name the rest of the block reads.
- **Order:** a flow deployed as units alone, with no text, has its file handlers in its
  composite's order, which follows hashes. `file_create`'s port becoming `Any` changed the order
  file-audit's test saw, so that test compares them as a set.
- **The sample, run live:** Python's `zipfile` and `gzip` made the deliveries (a deflated zip with
  a folder entry, a gzip with its file name and time). Each statement's account, closing balance
  and SHA-256 came back as `sha256sum` and the XML say, and `zipfile` read the archive
  (`testzip` clean, deflated, dated 1980-01-01, the statements byte for byte, the manifest). A
  route answered a statement with `content-encoding: gzip` to `curl --compressed` and plain
  without it. Not a zip, a zip bomb (200 MiB of zeros in 200 KiB), a gzip bomb (100 MiB) and a
  zip whose entry was `../evil.xml` were each kept as refused, with nothing archived. The export's
  17 runs verified. In the e2e test, Node reads the archive and the flow reads a zip Node wrote
  with its files stored.

#### 5.1.9 Slice 1.2e as designed (2026-11-08, against `3370c81`): JWT verify

An API that payment partners call checks who is calling: a bearer token, a JWT. An identity
provider signs it (RS256 from Auth0, Okta, Entra ID and Cognito; ES256 from Apple; PS256 and
ES256 under open banking's FAPI), or the partner itself does, with its own key or a secret it
shares with the API (HS256). Two leaves, after jose's `jwtVerify` and `decodeJwt`:

| Leaf | Gives | JavaScript |
|---|---|---|
| `jwt_verify(token, key, now) {issuer, audience, leeway}` | the token's claims, its payload as a record, when its signature verifies with `key` and they hold at `now`; else nothing | jose's `jwtVerify(token, key, { issuer, audience, clockTolerance, currentDate })`, jsonwebtoken's `verify` |
| `jwt_decode(token)` | `{header, payload}`, verifying nothing: what a token says before it is trusted, to pick the key that checks it | jose's `decodeJwt` and `decodeProtectedHeader`, jsonwebtoken's `decode(t, { complete: true })` |

**The key decides the algorithm.** The attack JWT libraries are known for is a token that names
its own: `none`, or HS256 with the issuer's RSA public key taken for the HMAC secret. Here a
token's `alg` only has to be one its key allows:

| `key` | Its algorithms |
|---|---|
| a text that isn't PEM, or bytes: a shared secret, at least the hash's length (32 bytes for HS256, 48 for HS384, 64 for HS512, as RFC 7518 requires) | HS256, HS384, HS512 |
| an RSA public key, 2,048 to 4,096 bits | RS256, RS384, RS512, PS256, PS384, PS512 |
| a P-256 public key | ES256 |
| an Ed25519 public key | `EdDSA`, and `Ed25519`, RFC 9864's name for it |

A public key is a PEM text (`-----BEGIN PUBLIC KEY-----`, or `RSA PUBLIC KEY`) or a JWK record. A
JWK's `alg`, when it has one, is the one algorithm it allows, and a JWK whose `use` isn't `sig`
or whose `key_ops` lacks `verify` allows none. A JWKS (`{keys: […]}`, what an issuer publishes at
its `jwks_uri`) gives the key whose `kid` is the token's; a token without a `kid` takes the set's
one key for its algorithm, and nothing when there are several, as jose does. `none` is no key's
algorithm. Any other key (a certificate, P-384, Ed448) gives nothing (fork 41).

**What else must hold.** The token is three base64url parts without padding: its header a JSON
object naming `alg`, without `crit` (no extension is understood, so none may be required), its
payload a JSON object. `exp` is required (fork 40) and `now` must be before it; `nbf`, when
present, not after `now`; each of `exp`, `nbf` and `iat` a number when present. `leeway` widens
both, in seconds: `0` (the default, as jose's), `30`, `60`, `120` or `300`, a family as
`rounding` is, so another value is refused when the flow compiles. Given `issuer`, `iss` must be
it; given `audience`, `aud` must be it or a list holding it. A JWE (five parts) gives nothing.

**`now` is an input**, as `stripe.verify` takes it: the leaf stays pure, so a rerun checks the
token at the time the run saw, and a token that has expired since still verifies in `holon
verify`. A flow passes `now` from `holon:clock`, in seconds as `exp` is.

**Taint.** A shared secret's taint stops at `jwt_verify`, as at `hmac_sha256`: what comes out is
the token's payload, nothing of the key (fork 42). The token's own taint goes on, derived, as
through any decoding.

**Dependencies** (fork 28): RustCrypto's `rsa` 0.9, `p256` 0.13 and `ed25519-dalek` 2.2, the
generation core's `sha2` 0.10 and `hmac` 0.12 belong to, each without its default features
(their `std` turns on `getrandom`, which doesn't build for the editor's wasm) and pinned with a
comment. HMAC is core's own, compared in constant time.

**The sample, `jwt-api`:** partners' services call a payouts API with tokens each signs with its
own key, as OAuth's `private_key_jwt` does. A person trusts a partner's public key by running an
action (`trust`), which keeps it under the partner's id: an agent can't add a partner unless a
person lets agents run it. A route reads the token's `iss` with `jwt_decode`, verifies the token
with that partner's key for the API's audience, and answers 401 with `www-authenticate: Bearer`
otherwise; a payout needs the `payouts:write` scope, else 403.

#### 5.1.10 Slice 1.2e as built (2026-11-08)

As designed, with what building it found:

- **Core:** `jwt.rs`; 183 primitives. Node's crypto (OpenSSL) wrote the test vectors, with the
  script that writes them kept beside them: each algorithm with its key as PEM and as JWK, a JWKS
  by `kid`, and the tokens that must give nothing (`none`, HS256 over the RSA public key, a
  1,024-bit RSA key, `crit`, no `exp`, an `exp` written as text, padding, a payload that is a
  list). RFC 7515's own HS256 example verifies at its time. An ES256 signature is JWS's 64 bytes,
  r then s, so a DER one gives nothing; Ed25519 verifies strictly, refusing a small-order key and
  a signature that isn't canonical. `rsa`, `p256` and `ed25519-dalek` build without `std`, and
  the editor's wasm builds without `getrandom`.
- **The language** needed no fix. A function of its own may read a collection, so the sample
  writes the token check once, as `caller`, for both routes (236 nodes, 256 with it written in
  each). Writing it met three things the language doesn't do yet, each written around: a
  top-level `const` binds only literals (the 401's message is a text, its record built where it
  is answered), `respond` isn't a branch of `?:` (its body and status are), and a `return` inside
  `if (x !== undefined)` isn't lowered.
- **The tests:** the server test trusts the OIDC tests' RSA key as a JWKS and a throwaway Ed25519
  key as a PEM, and mints tokens with `jsonwebtoken`, the crate the server already signs in with;
  the e2e test makes its keys and signs with Node, and trusts them from the editor's server panel.
- **A shared secret through `trust`** is kept in the collection and in the run's log, as any
  action's input is. The sample is written for public keys; a partner that shares a secret is
  given one as a deployment secret (`import key from "secret:…"`).
- **The sample, run live:** openssl made the partners' keys (an Ed25519 PEM; a P-256 key trusted
  as a JWK; RSA keys of 3,072 and 2,048 bits trusted as one JWKS; a secret of 64 characters and
  one of 16), each trusted with `holon action`, and Node signed the tokens. EdDSA, ES256, RS256,
  RS512, PS256, PS384, HS256 and HS512 verified, and so did an `aud` list holding the audience.
  401 for no token, `none`, HS256 over a partner's PEM or over its modulus, a token one partner
  signed naming another, a `kid` not in the set, no `kid` with two keys in it, ES256 against RSA
  keys, expired, `nbf` ahead, no `exp`, another audience, a payload changed under its signature,
  padding, `crit`, five parts, and the 16-byte secret. A payout was 403 without `payouts:write`,
  202 and then 409 for the same token, and 202 with no body. The export's 98 runs verified.

#### 5.1.11 Slice 1.2f as designed (2026-11-08, against `336a26b`): PGP encrypt and verify

A bank takes payment files encrypted to its public key, and signs what it sends back. Both halves
need only the bank's public key, which is data: encrypting to it and checking its signature are
leaves (R1). Decrypting with the company's own key, and signing with it, are the host's keys
(§5.3, step 1.5). Three leaves, after OpenPGP.js's `encrypt`, `verify` and `readKey`:

| Leaf | Gives | JavaScript |
|---|---|---|
| `pgp_encrypt(data, keys, random, now) {armor}` | the OpenPGP message: `data` (a text's UTF-8, or bytes) encrypted to each key's encryption key valid at `now`, as bytes, armored when `armor` is `"ascii"` (else `"binary"`, the default) | OpenPGP.js's `encrypt({ message, encryptionKeys, date, format })` |
| `pgp_verify(data, signature, keys, now)` | `{signer, key, created}` when one of `signature`'s signatures over `data` verifies with one of `keys`, valid when it was made and not after `now`: the primary key's fingerprint, the signing key's, and when it signed; else nothing | OpenPGP.js's `verify({ message, signature, verificationKeys, date })`, `gpg --verify file.sig file` |
| `pgp_key(key)` | `{fingerprint, user_ids, created, expires, revoked}`: what a person compares before trusting a key (`expires` absent when it doesn't) | `readKey` and its `getFingerprint()`, `gpg --show-keys` |

A key is a transferable public key, armored (`-----BEGIN PGP PUBLIC KEY BLOCK-----`, what
`gpg --export --armor` writes) or as bytes; `keys` is one or a list. A signature is a detached
one, armored or bytes, holding one or more signature packets.

**What is understood.** Version 4 keys of the two kinds GnuPG makes: RSA of 2,048 to 4,096 bits,
and Curve25519 (EdDSA to sign, ECDH over X25519 to encrypt, the "legacy" forms every OpenPGP
implementation reads). A key's subkeys count only with a binding signature from the primary that
verifies; a signing subkey only with its own back-signature too, so a subkey can't be claimed by
another key. A key, or a subkey, is used only while it is valid: created, not past the expiry its
newest self-signature gives, and not revoked by its primary. Its key flags decide what it does.
Other kinds (P-256 to P-521, Brainpool, DSA and ElGamal, version 6 keys) give nothing (forks 44,
45).

**Encrypting** writes what GnuPG reads everywhere: for each key, a public-key encrypted session
key packet to its newest valid encryption key (RSA with PKCS #1 v1.5; ECDH with the KDF and key
wrap its key packet names), then a symmetrically encrypted integrity protected data packet,
version 1: AES-256 in OpenPGP's CFB, holding a literal data packet (binary, no file name, dated
`now`) and the modification detection code (SHA-1) that refuses a changed message. AES-256
whatever the key prefers, since every implementation since 2005 reads it. No compression before
encryption (fork 47). Version 2 of that packet (AEAD, RFC 9580) waits for GnuPG to read it (fork
44).

**Randomness** (fork 29, decided): a message needs a fresh session key, a random prefix, and an
ephemeral X25519 key or PKCS #1 padding. A run's `random` is one number, 53 bits: too few. The
host now draws 32 random bytes for each run instead, recorded as `Random` is; `random` is a number
read from them, and `random_bytes` (`import { random_bytes } from "holon:random"`) is the bytes,
JavaScript's `crypto.getRandomValues(new Uint8Array(32))`. `pgp_encrypt` takes them as an input
and draws every random byte from BLAKE3 keyed with them over all its inputs, so two messages in
one run never share a session key, and the leaf stays pure: `holon verify` recomputes the
ciphertext byte for byte from the logged bytes. The export then proves what each encrypted file
held. Fewer than 32 bytes give nothing. Whoever reads the log can decrypt what the run encrypted,
and could already read it: its plaintext came into the run and is logged. A text computed from a
secret stays tainted through `pgp_encrypt`, as through any leaf, so it can't be written anywhere.

**Verifying** checks a version 4 signature of a binary document (type 0x00) or a text (0x01, its
line endings made CRLF), over SHA-256, SHA-384, SHA-512 or SHA-224; SHA-1 and MD5 signatures give
nothing (fork 48). The signature's issuer fingerprint, or key ID, names the key among `keys`; it
must be able to sign, valid at the signature's creation time, which must not be after `now`, and
the signature must not have expired at `now`.

**Dependencies** (fork 28, decided for OpenPGP): no `pgp` crate. rPGP brings some forty crates,
ciphers this refuses among them, and draws its own randomness, so its ciphertext couldn't be
recomputed. OpenPGP's packets, armor (with its CRC-24), key flags, the KDF and AES key wrap are
written here by hand, over primitives core already has or the workspace already builds: `rsa`,
`ed25519-dalek` and `sha2` from 1.2e; `curve25519-dalek` (X25519, already under `ed25519-dalek`);
`aes` 0.8 and `sha1` 0.10 (fingerprints, the modification detection code and older
self-signatures), each without default features. PKCS #1 v1.5's padding is written here too, over
`rsa`'s numbers, so every random byte is this code's. GnuPG checks it: its keys, signatures and
messages are the test vectors, and a test decrypts what `pgp_encrypt` writes with a decryptor the
GnuPG messages prove.

**The sample, `pgp-outbox`:** a payment file dropped in an outbox folder is encrypted to the
bank's key and written, armored, to the folder the bank collects from, standing for its SFTP
drop. A person trusts the bank's key by running an action (`trust`) that keeps it and answers its
fingerprint, to compare with the one the bank gave. The bank's signed answers (a file and its
`.sig`) land in a third folder; each is verified with the bank's key and kept as verified, with
when and by which key it was signed, or refused. What was sent is kept with the plaintext's
SHA-256 and the key it went to.

#### 5.1.12 Slice 1.2f as built (2026-11-08)

As designed, with what building it found:

- **Core:** `pgp.rs`, by hand as designed; 187 primitives (the three leaves, and `random_bytes`, a
  driver). GnuPG 2.2 wrote the test vectors, with the script that writes them kept beside them: an
  RSA-3072 key; a Curve25519 key with a signing subkey; one that expired, one revoked and another
  party's; signatures over SHA-1 to SHA-512, binary, text and armored, one made after `now`; and
  messages to each key and to both. A decryptor written for the tests reads GnuPG's messages, then
  reads what `pgp_encrypt` writes, and a test pins the bytes one set of inputs writes. A signing
  subkey whose back-signature is taken away signs nothing. `pgp_key` answers `revoked` too, beside
  what was designed, so a person trusting a revoked key sees it. The packets are written in the
  new format; GnuPG writes the old, and reads both.
- **Randomness:** the host draws 32 bytes for each run (`Random`), and core's `sourced_value`
  reads `random` from their first 53 bits, as the editor's replay does. Every run's sourced value
  changed with it, and so did the server's golden record.
- **The language:** `import { random_bytes } from "holon:random"`, printed back and described on
  hover as `random` is. Writing the sample met one thing: a record with a field that is nothing is
  nothing as a whole, so `trust` answers `key.expires ?? "never"` for a key that never expires. A
  trusted key is kept in the state with its fingerprint, and what isn't a key is refused inside
  `try`, nothing kept. The sample is 125 nodes; its folders are `payments` (read only), `upload`
  and `download` (read only).
- **Taint** is unchanged: what `pgp_encrypt` writes from a secret is derived from it, as any
  leaf's output is, since whoever holds the key reads it.
- **The tests:** the server test and the e2e test use the GnuPG vectors, so neither needs gpg on
  the runners. The bank's Curve25519 key is trusted; a payment file is encrypted to its encryption
  subkey, the message's key ID and algorithm those of GnuPG's message to it; its report verifies
  with the signing subkey; one another key signed is refused; and the runs verify.
- **The sample, run live,** against GnuPG 2.2.41 with keys made for it, a Curve25519 bank key and
  an RSA-4096 one, each trusted with `holon action`. gpg decrypted each file the flow encrypted to
  the same bytes: a pain.001 with CR LF lines, 300 KB of random bytes and an empty file to the
  Curve25519 key's ECDH subkey, and a pain.001 to the RSA key's subkey. The bank's binary, armored
  and text-mode EdDSA signatures verified, and, with the RSA key trusted, an RSA SHA-512 one.
  Refused: another key's signature, a report changed under its signature, a SHA-1 signature, a
  signature with no report beside it, and the Curve25519 key's signature once the RSA key was
  trusted. A junk key was refused, nothing kept. The bank's key, revoked by the certificate gpg
  stored when it made it, answered `revoked: true`, and a key made in January 2025 to expire 30
  days later answered its expiry; neither encrypted. The export's 29 runs verified, each message
  encrypted again byte for byte from its logged random bytes.

### 5.2 Email

**Sending.** `mail ops` names a mail resource an admin binds: SMTP (host, port, sender, login
from a secret source) or a provider's API (Postmark, SES, Resend: HTTPS underneath, the same
item). `ops.send({ to, subject, text, attachments })` sends once the run's writes land, at most
once per run. The item names who may receive:

```
it may:
  send mail through "ops" to *@acme.com, treasury@bank.example
```

The recipients are part of the grant: a deploy whose text can reach another address (a recipient
computed from a request) asks for `to: *`, which the policy may refuse. A body computed from a
secret is refused, as a channel pack is.

**Receiving.** `on mail ops (m) { … }` runs once per message: `m.from`, `m.to`, `m.subject`,
`m.text`, `m.attachments` (kept in `blobs.rs`, like a file's text) and **`m.authenticated`**,
the sender's SPF, DKIM and DMARC result. A `From:` line proves nothing; a flow that acts on mail
checks `m.authenticated` first, and the sample does. The message is moved to a folder of the
mailbox (or flagged) after the run commits; its `Message-ID` is the key that keeps it from running
twice.

Where it comes from is the binding's business: IMAP (any mailbox), or a provider's inbound
webhook into a route the host owns. Microsoft 365 has turned passwords off for IMAP and Google
steers to OAuth, so those mailboxes need it (§8, fork 2).

The pair is the approval loop payment teams already run on: a payout over a limit mails an
approver a resume link (`approved-payout` already waits on one), and the run continues when it is
used.

#### 5.2.1 As designed (2026-11-08, against `a781416`)

The text above is in the language as it was. In TypeScript a mail resource is imported as a folder
or a database is, sends with a method, and is listened to as a folder is:

```ts
import ops from "mail:ops"

ops.send({ to: "approvals@acme.com", subject: `Payout ${id}`, text: `…` })
on.mail(ops, (m) => { … })
```

Three slices, each with its sample:

| Slice | What | Sample |
|---|---|---|
| 1.3a | sending through SMTP: the binding, recipients in the grant, delivery after the run, attachments | `approval-mail` |
| 1.3b | receiving through IMAP: `on.mail`, the message parsed, `m.authenticated` from the mailbox's own verdict, moved after the run | `remittance-mail` |
| 1.3c | mailboxes that log in with OAuth (fork 2): Microsoft 365, Google Workspace | `support-inbox` |

SMTP and IMAP come first because every mailbox and every sending provider speaks them: Postmark,
SES, Resend, SendGrid and Mailgun take SMTP with a login, as Fastmail, Gmail with an app password
and a company's own relay do. A provider's API is HTTPS underneath and the same item (1.3c).

#### 5.2.2 Slice 1.3a as designed (2026-11-08): sending

**The binding.** An admin binds a mail resource as a database is bound (`--mails`, owner-only;
`/admin/mails`; `holon mail add|remove|list`), for the whole server or one deployment (`--for`):

```
holon mail add ops --smtp env:HOLON_SECRET_OPS_SMTP --from "Treasury <treasury@acme.com>" --to "*@acme.com,treasury@bank.example"
```

- `--smtp` names a source (`env:…`, `vault:…`) holding the server's URL with its login, as a
  database's connection is held: `smtps://user:password@smtp.example.com` (TLS from the first
  byte, port 465 unless given) or `smtp://user:password@smtp.example.com:587` (STARTTLS, required:
  a server that doesn't offer it is refused, so a login never crosses in clear). Plain SMTP only to
  a loopback address: a relay on the machine, the tests' sink. The URL is never stored, shown or
  logged; the host keeps the address without the login. AUTH PLAIN or LOGIN, whichever the server
  offers; none when the URL has no login.
- `--from` is the sender, the admin's, never the flow's: a flow can't send as someone else.
- `--to` is every address the binding may ever send to, the ceiling: addresses and `*@domain`, or
  `*` for anyone. It is required: a binding that sends anywhere says so.

**The flow.** `import ops from "mail:ops"`, imported under its own name as a database is, then
`ops.send({ to, cc, subject, text, html, attachments })`: `to` and `cc` an address or a list (one
recipient at least, 50 at most), `subject` and `text` texts, `html` optional, `attachments` a record
of a file name to its text or bytes, as `zip` takes one. It lowers to a `mail_send{mail, to}` leaf,
an effect as `channel_send` is. Its `to` parameter is the recipients the text names, when each is
written as text in quotes, else `*`: the recipients are part of the grant, so they are read off the
text as a request's host is.

**The grant.** `MailSend("ops/approvals@acme.com")` for each recipient written in the text, or
`MailSend("ops/*")` when one is computed (from a request, a database), two names in one as
`DbWrite("db/table")` carries them:

```
it may:
  send mail through "ops" to approvals@acme.com
  send mail through "ops" to an address it computes, within its binding's
```

A deploy is refused when a recipient written in the text is outside the binding's `--to`, or the
binding isn't there ("the mail "ops" isn't bound on this server", as a folder's). As each mail
goes, every recipient must be one the grant names (or the grant says `*`) and inside the binding's
`--to` as it is then; a mail with one that isn't is refused whole, and the refusal is noted in the
deployment's activity.

**The message** is the host's to write: `From` the binding's; `To`, `Cc` and `Subject` the flow's
(RFC 2047 words when not ASCII); `Date` the run's clock; a `Message-ID` from the run's id and the
send's place in it (`<run-…@from-domain>`), so a mail sent again is known for the same one; the
text as UTF-8, quoted-printable; with `html`, both as `multipart/alternative`; with attachments,
`multipart/mixed`, each base64 with its type from its extension. No `Bcc`, no `Reply-To` (fork
51).

**Delivered after the run,** as a channel's pack is: once the run's writes have landed and its
record is on disk, never from an attempt that went round again. A queue for each binding sends in
order on its own thread, so a slow server holds no run. A temporary failure (no connection, a 4xx)
is tried again with the same `Message-ID`, after a minute, then 5, then 30, then each hour for a
day; a permanent one (5xx), or a day of temporary ones, gives up. Each outcome is noted in the
deployment's activity, and a delivery record beside its log says what was delivered, so a restart
sends what wasn't, as held packs are rebuilt from the log (fork 50).

**Logged:** the run's `performed` keeps each message, its recipients, subject and text, and its
attachments by size and hash, as a written file is; `holon verify` computes it again.

**Secrets:** a message holding a secret or an answered token, or computed from a secret, is
refused, as a channel's pack is.

**Limits:** 50 recipients and 10 MiB a message, attachments included, 100 mails a run.

**Links.** The host knows no address of its own the world reaches, so a flow writes the base of a
link it mails as text (fork 52). A link that approves is a page whose button approves: a mail
scanner opening each link (Outlook's, Gmail's) must not approve anything.

**Mail before an `await`** waits for the run's end, as its other effects do: an approval link is
mailed by the route that starts the waiting run, which knows its token (`started.waiting.approved`),
as approved-payout answers it (fork 53).

**Dependencies:** none new. SMTP's commands and MIME's parts are written here, over the `rustls`
and `webpki-roots` the database connections use (fork 50).

**Tests:** an SMTP server in the server test, and one the e2e server starts, each keeping what it
is sent, plain on loopback.

**The sample, `approval-mail`:** a payout over 1,000 waits for an approval. The route that takes
it starts the payout's run and mails approvals@acme.com a link, with the payout attached as a CSV;
the link's page has a button, and its POST approves. The approved payout is recorded, and the one
who asked for it (an address from the request) is mailed that it was approved: a computed
recipient, held to the binding's `*@acme.com`.

#### 5.2.3 Slice 1.3a as built (2026-11-08)

As designed, with what building it found:

- **Core:** `mail.rs` and the `mail_send` leaf, an effect; 188 primitives. The message is made
  regular in core, so the log and the grant read one shape: `to` and `cc` lists, `cc` only when
  there is one, `html` and `attachments` only when given. A message that isn't one has no value,
  so nothing is sent: a field it doesn't know (`bcc`), no recipient or more than 50, an address
  that isn't one or that the leaf's `to` doesn't name, a subject with a line break, an attachment
  that isn't a text or bytes or whose name isn't a file's, more than 10 MiB. MIME is written there
  too, a pure function of the message, the sender, the run's clock and the `Message-ID`
  (`<run.n@from-domain>`): headers folded at 78 characters, RFC 2047 words, quoted-printable
  text, base64 attachments in 76-character lines with an RFC 2231 name when it isn't ASCII, and
  boundaries with `=_` in them, which neither encoding writes.
- **The language:** `import ops from "mail:ops"`, imported under its own name, printed back, and
  described on hover. The recipients are read off the call's record when it is written out with no
  spread and each of `to` and `cc` is text in quotes or a list of them; else `*`. Where the record
  is written out, `holon check` refuses a field it doesn't know, a missing `to`, `subject` or
  `text`, a quoted recipient that isn't an address, and none or more than 50.
- **The binding:** as designed; `--mails` is `mails.json` in the data folder. The admin page's
  resources tab binds and unbinds a mail (where its URL is kept, never the URL; its sender; its
  `--to`), and its permissions list each mail's holders by recipient: "mails approvals@acme.com,
  addresses it computes".
- **Delivery:** a spool, not the delivery record fork 50 named. Each mail is a file in
  `mail-spool` beside the bindings (owner-only), written before its run's record is and outside
  the log's lock, held until that record is on disk, then queued. A restart sends each file whose
  run its log holds, and deletes one whose run a crash lost: what the log says was sent is what
  goes. An unrecorded run's mail is spooled as unrecorded and queued at once. One thread a binding
  sends, in order; the outcome is a timeline event of kind `mail`: `mailed "…" to … through ops`,
  or `gave up mailing …`.
- **Refusals:** a recipient the grant doesn't cover is caught as the run's effects are checked, as
  any ungranted effect is, in the server's log; a recipient the binding no longer allows is caught
  as the mail goes, and given up on the timeline. A secret in a mail, or a value computed from one,
  is refused as a channel's pack is. A run past 100 mails fails, `mail not sent: …`.
- **SMTP** by hand as designed (`smtp.rs`): implicit TLS or STARTTLS over `rustls` with
  `webpki-roots`, refusing STARTTLS when the server says more before TLS begins; `EHLO` with the
  local address as a literal; AUTH PLAIN, else LOGIN, the login zeroized and an AUTH error never
  echoing what was sent; dot-stuffing. A connection failure or a 4xx is temporary, a 5xx
  permanent.
- **The sample:** every payout waits for a second person's approval (four eyes), not only one
  over 1,000: the sample shows the loop, and a threshold is one `if`. The approval is recorded with
  `approved_via`, the method of the request that approved it, so a page opened by a scanner (a
  `GET`) is told apart from the button (`POST`) in the record too. The page's route is
  unrecorded: the live run showed its `GET` logged with the token in its path, which a log reader
  could approve with, where the route that resumes logs `‹token:approved›`. A host that kept a
  waiting run's tokens out of every logged path would make the option needless; until then the
  sample says it.
- **The tests:** the server test's SMTP sink takes AUTH PLAIN and LOGIN; the deploy is refused
  until `ops` is bound; the link mailed opens a page and approves nothing until its button is
  pressed; an asker outside `*@acme.com` is given up, on the timeline; the log keeps the CSV by
  hash. The e2e server starts a sink of its own (`editor/e2e/smtp-sink.mjs`) that writes each mail
  it takes as a file; the test approves through the mailed link in the browser, then reads the
  binding and its holder on the admin page.

#### 5.2.4 Slice 1.3b as designed (2026-11-08, against `c9f4017`): receiving

**The binding.** The same mail resource receives when it names a mailbox: `--imap` beside or
instead of `--smtp`, so one name can do both (`ops.send(…)` and `on.mail(ops, …)`):

```
holon mail add remit --imap env:HOLON_SECRET_REMIT_IMAP --folder INBOX --done Processed --verdict-from mx.acme.com
```

- `--imap` names a source holding the mailbox's URL with its login, as `--smtp` does:
  `imaps://user:password@imap.example.com` (TLS from the first byte, port 993 unless given) or
  `imap://…:143` (STARTTLS, required; plain only to a loopback address). Never stored, shown or
  logged; the host keeps the address without the login.
- `--folder` is the folder read (`INBOX` unless given); `--done` the one each message is moved to
  once its run is over (`Processed` unless given), made when the mail is bound if it isn't there.
  ASCII names (IMAP writes others in its own UTF-7).
- `--verdict-from` names whose verdict on the sender counts: the `authserv-id` the mailbox's own
  server writes first in its `Authentication-Results` header (`mx.google.com`,
  `mx3.messagingengine.com`). Without it nothing is authenticated: a header anyone can write
  isn't believed by default.
- `--every` is how often the folder is looked at, `1m` unless given, `10s` at least.
- `--from` and `--to` stay `--smtp`'s; a binding with only `--imap` sends nothing.

The server logs in when the mail is bound, selects the folder and makes `--done`.

**The flow.** `on.mail(remit, (m) => { … }, { log })`, `remit` imported from `"mail:remit"`: a
run for each message that comes to the folder, one handler a mail in a flow. It lowers to an
`on_mail{mail}` node, a template as `on_file` is, whose `message` output is a `mail_arrival{mail}`
leaf, sourced; the server declaration's child carries `declare = "on_mail"` and `mail`, and
the host reads it as it reads a file handler (`MailHandler`, labelled `on mail remit`).

**The message,** `m`, made in core from the message's bytes (`mail::received`), a pure function
of them and the binding's `--verdict-from`:

| Field | What |
|---|---|
| `from` | the sender's address, as `From:` writes it (one address, else nothing) |
| `name` | the sender's display name, `""` when none |
| `to`, `cc` | lists of addresses |
| `subject` | RFC 2047 words decoded |
| `date` | `Date:` in seconds since the epoch, as `now` is; nothing when it isn't a date |
| `message_id` | without its `<>`; for a message with none, `sha256:` and the hash of its bytes |
| `text` | the first `text/plain` part that isn't an attachment, decoded; `""` when none |
| `html` | the first `text/html` part, when there is one |
| `attachments` | a record of each attachment's file name to its content: text for `text/*`, `application/json`, `application/xml` and `application/csv` in a charset core reads, bytes otherwise |
| `authenticated` | `true` when the verdict says `dmarc=pass` for the `From:` domain |
| `verdict` | `{ spf, dkim, dmarc }` as that verdict writes them, `"none"` for each it doesn't |

`From:` proves nothing; `authenticated` is the mailbox server's word that the domain in `From:`
sent it (DMARC passes only when SPF or DKIM passed for that same domain). A flow that acts on
mail checks it first, and the sample does. Only the first `Authentication-Results` header whose
`authserv-id` is the binding's is read, the one the mailbox's server wrote on arrival: one a
sender wrote further down, or under another name, is never believed. Charsets: UTF-8, US-ASCII,
ISO-8859-1 and Windows-1252; another is read as UTF-8, what isn't kept as U+FFFD. Transfer
encodings: 7bit, 8bit, binary, quoted-printable, base64. Two attachments of one name: the second
`name (2).ext`.

**The grant.** `MailArrival("remit")`: "is run by each mail that comes to remit, read and moved
there". A deploy is refused while `remit` isn't bound with `--imap`, and while another deployment
listens to the same mailbox (a message is moved once, so one listener a mailbox).

**Watching.** A thread looks at each running deployment's mailboxes when due (`--every`): it logs
in, selects the folder, lists its UIDs (`UID SEARCH ALL`), and for each it hasn't seen, oldest
first, at most 20 a look, fetches the message (`UID FETCH … BODY.PEEK[]`, nothing marked read) and
starts one run. What it has seen is kept beside its log (`mail-seen/<slug>.json`), with the
folder's `UIDVALIDITY`:

- **New since the deploy.** What the folder holds when the deployment first looks is seen, not
  run on and not moved, as a folder's files are: a mailbox that already holds a thousand messages
  isn't worked through by a flow deployed to it.
- **Moved after the run.** Once the run's record is on disk (or it is over, for an unrecorded
  handler), the message is moved to `--done` (`UID MOVE`; without `MOVE`, `UID COPY`, `\Deleted`
  and `UID EXPUNGE`; without `UIDPLUS` as well, left marked `\Deleted`). A run that failed is
  moved too: its record says why, as a file's failed run doesn't run again.
- **Never twice.** A message whose run is over is noted with its UID before it is moved, so one
  whose move failed, or a crash cut off, is moved at the next look, not run again. A message
  whose `message_id` is one of the last 1,000 run on (a second copy) is moved without a run, on
  the timeline.
- **Too large:** a message past the deployment's `max_body_bytes` (10 MiB) is moved without a
  run, on the timeline, as a request past it is refused.
- **Unreachable:** a look that can't log in, or loses the connection, is tried at the next one;
  the timeline says so once, and again when it reads the mailbox again. A changed `UIDVALIDITY`
  (the server numbered the folder again) sees the folder anew: what it holds then isn't run on.

**The run** is the handler's alone, logged under its label with `kind: "mail"`, its message
sourced as `MailArrival("remit")`. **Logged** whole, attachments included, as a request's body is
(under the same 10 MiB); `holon verify` and the editor's replay read it from there. A secret in a
message isn't possible: it comes from outside, as a request does.

**Dependencies:** none new. IMAP's commands by hand (`imap.rs`), over `rustls` and `webpki-roots`
as SMTP is; MIME read in core beside where it is written (fork 54).

**Tests:** an IMAP server in the server test, keeping its folders in memory; the e2e server starts
one of its own (`editor/e2e/imap-server.mjs`) serving a folder of `.eml` files, `INBOX` and the
others as folders beside it.

**The sample, `remittance-mail`:** customers mail remittance advice to remittances@acme.com, each
a CSV of `invoice,amount` attached. Each authenticated mail's lines are matched against the open
invoices (put by `POST /invoices`): an invoice paid in full is closed, with who paid it and the
message it came in; a line that matches nothing, or not the amount, is kept to look at. A mail
that isn't authenticated changes nothing and is kept with its verdict.

#### 5.2.5 Slice 1.3b as built (2026-11-09)

As designed, with what building it found:

- **Core:** `mail::received` (`mail/received.rs`), MIME read by hand (fork 54): headers unfolded,
  parts to a depth of 20, RFC 2047 words and RFC 2231 parameters (continuations and charsets),
  quoted-printable and base64 read leniently, UTF-8, ISO-8859-1 and Windows-1252 (a BOM dropped),
  `Date:` with the obsolete zones. Whatever the bytes, it gives a message: a part it can't read is
  kept as bytes or left out, never a failed run. The `mail_arrival{mail}` leaf, sourced; 189
  primitives. `MailArrival` is a driver, and a server's `mails` are read from its `on_mail`
  children, one a mail (`DuplicateMailHandler`).
- **The language:** `on.mail(remit, (m) => …, { log })` as designed; refused when its first
  argument isn't an imported mail, when a mail has two handlers, and at a `return` with a value (a
  message's run answers nobody). The printer writes it back, the mail imported as one sent
  through is. `holon.d.ts` gives `m` a `MailMessage` type, so the editor checks what a handler
  reads.
- **The binding:** one binding holds both halves: `smtp`, `from` and `to` empty when it only
  receives, `mailbox` absent when it only sends. A flow that sends through a mail bound only with
  `--imap` is refused ("isn't bound to send"). One listener a mailbox is checked against the
  mailbox itself (the source its URL is read from, and the folder), so a second name bound to the
  same folder is refused too. The admin page binds a mailbox beside the SMTP fields and lists it;
  permissions say "starts on mail it receives".
- **Watching** (`inbox.rs`): a thread looks every second for the mailboxes due, each on its own
  thread, the timing in memory and what each deployment has seen in
  `mail-seen/<slug>/<mail>.json` (one file a mail, so two handlers of one deployment never write
  one file). What the folder holds is seen at the watcher's first look, not at the deploy, which
  doesn't log in to the mailbox: a message that comes in the second between them is seen, not run
  on. That first look is on the timeline, `listens to the mailbox of remit (imaps://… INBOX), what
  it holds now (3 messages) isn't run on`, which says why those aren't, and which the e2e test
  waits for. "Too large" is the data plane's 10 MiB, a server's limit rather than a deployment's.
  A server without `MOVE` and `UIDPLUS` has the message copied and marked `\Deleted`, and it isn't
  copied again.
- **The run** is the handler's alone, as a file handler's: `kind: "mail"`, its label the route,
  the message sourced as `MailArrival("remit")` with the clock and the random bytes. A parked one
  keeps the message among its sourced values (`durable.rs`), and `holon verify` reruns it as that
  handler alone.
- **IMAP** by hand as designed (`imap.rs`): `AUTHENTICATE PLAIN` (with the initial response when
  `SASL-IR` is offered), else `LOGIN`; the capabilities read again after STARTTLS and after the
  login; a literal read whole wherever a response holds one, so a message whose bytes look like
  one (`{5}`) reads as it is. smtp.rs's TLS, URL and base64 helpers are shared.
- **The sample** found that `{ ...state.open, [id]: … }` keeps nothing while `state.open` isn't
  set (a spread of nothing is nothing, §5.5 of the language): it writes `...(state.open ?? {})`.
- **The editor:** the server panel lists mail handlers; a mail run is titled `mail from
  billing@customer.example: Remittance May (on mail remit)`; replay shows the message on the
  `on_mail` node; the palette has `mail_arrival`.
- **The tests:** an IMAP stand-in in memory (`imap::stand_in`, with `MOVE` and without); the
  binding refused for a bad source, login, folder, `--done`, `--every` and `--verdict-from`, and a
  second listener refused under either name; the server test deploys the sample, refused until
  `remit` is bound, sees the message there before it, then runs an authenticated message (an
  invoice settled), one whose verdict another server wrote (refused), and a second copy (moved
  without a run), all three moved to `Processed`, and verifies the runs. The e2e server starts an
  IMAP server of its own (`editor/e2e/imap-server.mjs`) whose folders are folders of `.eml` files;
  the test drops two messages in, waits for the invoice to be paid and both files to move, verifies
  the export, and reads the mailbox on the admin page.

#### 5.2.6 Slice 1.3c as designed (2026-11-09, against `c853e1d`): mailboxes that log in with OAuth

Microsoft 365 has turned passwords off for IMAP and SMTP, and Google Workspace steers to OAuth.
Both still speak IMAP and SMTP: they take an OAuth token in place of a password (SASL `XOAUTH2`).
So fork 2 is decided for the host doing OAuth: the same binding, the same servers, the same
flows, logged in with a token the host asks for. Microsoft Graph and the Gmail API stay in Tier 3,
for a tenant that has turned IMAP itself off.

**The binding.** `--oauth` names a source holding the token request as a URL, the client's id and
secret as its login, as the other sources hold a server's URL with its login:

```
HOLON_SECRET_SUPPORT_OAUTH=https://<client id>:<client secret>@login.microsoftonline.com/<tenant>/oauth2/v2.0/token?scope=https://outlook.office365.com/.default
HOLON_SECRET_SUPPORT_IMAP=imaps://support%40acme.com@outlook.office365.com
HOLON_SECRET_SUPPORT_SMTP=smtp://support%40acme.com@smtp.office365.com:587
holon mail add support --imap env:HOLON_SECRET_SUPPORT_IMAP --smtp env:HOLON_SECRET_SUPPORT_SMTP
  --oauth env:HOLON_SECRET_SUPPORT_OAUTH --from "Support <support@acme.com>" --to "*" --verdict-from …
```

- **Which grant:** client credentials (`scope=` in the query: Microsoft 365's app-only access, no
  person's consent, nothing that runs out), or a refresh token (`refresh_token=` in the query, a
  person's consent given once: Google's `https://oauth2.googleapis.com/token`, scope
  `https://mail.google.com/`). Values in the query percent-encoded.
- **Which login:** a server's URL with a password logs in with it; one with a user and no
  password logs in as that user with the binding's token. `--oauth` where neither URL names a
  user without a password is refused, and so is a user without a password and no `--oauth`. So
  one binding may send through a relay's password and receive through Microsoft 365's token.
- **Where:** the token URL is `https://` (plain `http://` only to a loopback address, the tests').
  Never stored, shown or logged; the host keeps the endpoint without its login or query.

The server asks for a token when the mail is bound, and logs in with it to each server, so a
wrong secret or a mailbox the app may not read is refused then.

**The token.** Asked for with a form POST (`grant_type=client_credentials` with the scope, or
`refresh_token`; the client's id and secret in the body, which Microsoft and Google both take),
kept in memory only, until a minute before it runs out (`expires_in`), and by the source's value,
so a value an admin changes asks anew. A login refused with it drops it: the next asks for a new
one. A refresh token the answer replaces is kept in memory in place of the source's for the
server's life; a restart starts from the source's again. A provider that revokes a refresh token
once it is used isn't one this host can hold (it would write the new one back to the source):
Microsoft 365 runs on client credentials, which have none, and Google's refresh tokens are
reused.

**The login.** SASL `XOAUTH2`: `user=<user>^Aauth=Bearer <token>^A^A`, base64. IMAP sends
`AUTHENTICATE XOAUTH2` when the server offers `AUTH=XOAUTH2` (the response with the command when
it offers `SASL-IR`); SMTP sends `AUTH XOAUTH2` when it offers it. A refusal may come as a
challenge first (a status in JSON, Google's way): it is answered empty, as the mechanism says, and
the login is refused.

**Failures.** The token endpoint unreachable, or answering 5xx: temporary (the mail is tried
again, the mailbox at its next look). A 4xx: permanent, saying its `error` (`invalid_client`,
`invalid_grant`) and the first 200 characters of its `error_description`, which hold no secret.
Each on the timeline as a password's refusal is.

**The grant.** Nothing new: `MailSend` and `MailArrival` as before. How the host logs in is the
admin's business, as a password is.

**The admin page and the CLI:** an OAuth source beside the servers' (`--oauth`); a binding is
listed with "logs in with OAuth (https://login.microsoftonline.com/…/token)".

**Dependencies:** none new. The token request goes through `ureq`, as the OIDC sign-in's do.

**Tests:** a token endpoint in the server test (it counts what it is asked); the IMAP stand-in
and the SMTP sink take `XOAUTH2`. The e2e server starts a token endpoint of its own, and its IMAP
server and SMTP sink take the token it gives.

**The sample, `support-inbox`:** a support mailbox on Microsoft 365, received from and answered
through one binding that logs in with OAuth. Each authenticated request opens a ticket, numbered,
and is answered with its number; one that isn't authenticated is kept, unanswered (answering a
forged `From` would mail whoever it names).

#### 5.2.7 Slice 1.3c as built (2026-11-09)

As designed, with what building it found:

- **A refused token is tried again at once.** The design dropped a token a login was refused
  with, for the next attempt to ask anew; built, the same login is tried once more with a new one
  (`Mails::with_token`), so a token revoked before it runs out costs one more login, not a failed
  look or a mail waiting for its retry. A second refusal is the login's failure, the new token
  dropped too.
- **One token for both servers:** kept by the blake3 of the request's value, so a binding with
  both halves asks once (the server test counts what the endpoint is asked).
- **The login, shared:** `smtp::Login` (a password, or a user and the token given at login time)
  is what both clients parse from a URL's login, and `smtp::xoauth2` writes the SASL response for
  both. IMAP sends `AUTHENTICATE XOAUTH2` only when `AUTH=XOAUTH2` is offered (with the response
  when `SASL-IR` is, else after the `+`), answers a challenge empty and reads the tagged `NO`, and
  reads the capabilities again after; SMTP's `AUTH XOAUTH2` answers a `334` empty the same way,
  and a 4xx is temporary. A server that offers no XOAUTH2 to a token login is refused, saying so.
- **The request's URL is checked part by part** (`oauth::Grant`): a login that is the client's id
  and secret, a host, `https://` (or `http://` to a loopback address), and a query of `scope` and
  `refresh_token` only, any other key refused, so nothing the endpoint isn't sent hides there. Its
  errors name the part, never the value.
- **The watcher logs in as the binding does:** `inbox.rs` opens the mailbox through
  `Mails::open_mailbox`, so a look that needs a token gets it from the same place, kept.
- **The binding shows** `"oauth": {"source", "address"}`, the endpoint without its login or query;
  `holon mail list` and the admin page say "logs in with OAuth (<endpoint> from <source>)", and
  the admin page has the OAuth source beside the servers'.
- **The tests:** a token endpoint stand-in in memory (`oauth::stand_in`: it counts what it is
  asked, refuses a wrong client as Entra does, with `AADSTS7000215`, and replaces a refresh token
  each time it is used); the IMAP stand-in logs in by token alone (`start_with_token`), the SMTP
  sink takes `XOAUTH2` and can be made to refuse a token. Unit tests: the URL's parts, a token
  kept until a minute before it runs out and asked for again, a rotated refresh token used next,
  an endpoint that isn't there temporary, each client's XOAUTH2 login and refusal. The server
  test binds `support` with `--oauth`, deploys the sample, runs an authenticated request (a
  ticket, its answer sent logged in with the token) and a forged one (kept, unanswered), both
  moved to `Answered`, the token asked for once; then the SMTP server refuses that token, and the
  next answer goes with a new one. Neither the binding nor the log holds the secret or a token.
  The e2e server starts a token endpoint (`editor/e2e/token-endpoint.mjs`), and a second IMAP
  server and SMTP sink that take its token only; the test drops two requests in, waits for the
  ticket, reads the answer and the user it was sent as, sees both files moved, and reads the
  binding on the admin page.

### 5.3 Keys

A **key** is a resource an admin binds: a PGP private key, later a key in a cloud KMS or an HSM,
its material from a secret source. A flow names it (`key bank`) and asks the host to use it:
`bank.decrypt(file)`, `bank.sign(text)`. The material never enters the run, so the taint rule has
nothing to follow; the item is `KeyUse("bank", decrypt)`. What a decrypt returns is ordinary data,
logged like a file read (by hash, kept in `blobs.rs`), so a replay needs no key.

Encrypting to a bank's public key and checking its signature are leaves (R1): the public key is
data.

### 5.4 Client certificates

Bank and payment-network APIs often require a client certificate, and many sit behind a private
CA. An **identity** is a resource an admin binds: a certificate, its key (from a secret source),
and the CA certificates to trust instead of the public roots. A flow's call names it,
`http.post("api.bank.example", …, identity: bank)`, and the grant pairs them:
`HttpRequest("api.bank.example")` with `ClientIdentity("bank")`. The key never enters the run.
`outbound.rs` builds the TLS connection with it; rustls takes a client certificate.

### 5.5 Queues

Publishing is an effect (`QueueSend("events/payouts")`, sent once the run's writes land);
consuming is a handler (`on message events (m) { … }`), acknowledged after the run commits, so
delivery is at least once and the message id, kept in the claims table, makes it once. One engine
comes in Tier 1 (§7, step 1.6), which one is §8's fork 8: SQS shares S3's request signing (§6),
RabbitMQ and NATS have pure-Rust clients, Kafka's mature client is C (librdkafka).

Designed with the feeds, in step 1.6 (§5.9.1): SQS's API first.

### 5.6 Approvals

Not an outside system: Holon's own. Today `await` waits for a resume token, and whoever holds the
link continues the run. A first-class approval waits for **a person in a role**:
`approved = await approval(role: "payments-approver", expires: 1d)`. The editor shows each
approver what waits for them; approving is a signed-in action, recorded in the run's log with who
and when, and `holon verify` reports it. Four eyes (the approver isn't who started it) and *n* of
*m* are options on it. The item says who may approve:

```
it may:
  wait for approval by "payments-approver", up to 1 day
```

This is the capability closest to Holon's pitch: agent governance with evidence a reviewer can
check offline.

### 5.7 Commands and persistent terminals

Payment operations still run programs: a bank's command-line client, a reconciliation script, a
vendor's export tool, and sometimes an interactive system no API reaches (a vendor CLI that logs
in once and takes commands, an SSH session to a legacy box, a mainframe screen). And agents ask
for a terminal more than for anything else. R5 says no shell; these two capabilities are what
stands in its place: the host runs what an admin bound, and the run's exchange with it is logged
like any other input.

**A command** runs a program once and answers with what it printed. An admin binds it:

```
holon command add reconcile /opt/bank/reconcile --arg "--date {date}" --arg "--account {account}" \
  --dir /srv/recon --env BANK_TOKEN=env:HOLON_SECRET_BANK_TOKEN --timeout 2m
```

- **The program and its arguments are the admin's.** A flow names `command reconcile` and calls
  `reconcile.run({ date: d, account: a })`; each `{placeholder}` is one argument, typed (text,
  number, a date), passed as its own argv entry, never through a shell, so a value can't add an
  argument or a command. A value that starts with `-` is refused unless the binding allows it.
- **The item names it:** `CommandRun("reconcile")`, shown as "run the command reconcile
  (/opt/bank/reconcile)". Its environment's secrets come from a secret source into the process,
  never into the run.
- **Its answer is the run's input:** `{ status, stdout, stderr }`, logged (large output kept in
  `blobs.rs`), so a replay and `holon verify` use the log and run nothing. It runs between rounds,
  as an outbound request does, and a run that runs again answers it from its record (`prior`):
  once per run.
- **Its effects are its own.** Holon can't know what a program changed, so a command is treated as
  an outbound call that can't be undone: claims hold around it as around a request, and the grant
  says so.
- **Limits:** a timeout (the process tree is killed, a Windows job object or a process group),
  output bytes, commands at once per deployment. The process runs as the server's user, or as
  another an admin names (§8, fork 9).

**A terminal** is an interactive program the host keeps running between runs: a pseudo-terminal
(ConPTY on Windows, a pty elsewhere) or an SSH session with its host key pinned in the binding.

```
holon terminal add ledger --ssh ops@ledger.bank.example --host-key SHA256:… \
  --login-secret ledger_password --idle 30m
```

- **A run holds it for its whole length,** as a run lock does (databases §7.3): one run at a
  time, so two runs never type into one session. Others wait (bounded) or are turned away busy.
- **A flow sends text and waits for an answer:** `ledger.send("BAL 4471")`, then
  `ledger.expect("READY>", timeout: 10s)` answers the screen text up to the match. A password goes
  in as `ledger.send_secret("ledger_password")`: the host types it, the run never holds it, and
  the transcript holds the secret's mark (`‹secret:ledger_password v3 #a1b2c3d4›`, its version and
  fingerprint, `recorder.rs`), never its value.
- **The transcript is the run's input.** What the run sent and what came back, in order, is in its
  log; a replay answers each `expect` from it, and `holon verify` checks the run's computation from
  it. What verify can't check is the session's state before the run: the log says so.
- **The whole session is recorded,** run after run (kept in `blobs.rs`, by deployment and run), so
  an admin sees who typed what into the box and when. For an agent driving a terminal, this is the
  evidence the pitch promises.
- **The item:** `TerminalUse("ledger")`, shown as "type into the terminal ledger (ssh
  ops@ledger.bank.example)".
- **Its weaknesses, stated:** state outside any run (a session that changed between runs replays
  nothing of that change), screen-scraping that breaks when the program's output changes, and one
  run at a time. So a terminal is the **last resort**: an API first, a command second.

**Agents.** An agent through MCP gets no shell with the server's reach either. What it gets is
the commands and terminals a grant lets it use, as tools, each exchange logged as a run's would
be, and, where it has to work freely, a shell in a sandbox (§5.8).

A mainframe screen protocol (TN3270, 5250) is a terminal backend of its own, when a job brings
one. The same SSH engine (russh, pure Rust) serves terminals and the SFTP backend (§6); a
pseudo-terminal's is portable-pty. A WebSocket or a raw socket is a terminal backend too, its
`expect` matching whole messages instead of screen text (§5.9).

### 5.8 A shell in a sandbox

A command describes one program; some work can't be written down that way. An analyst's
one-off script, a reconciliation that needs `python3`, `gpg` and `csvkit` in a row, an agent
that has to look around before it knows what to run. For that, the manifest describes the
**walls** instead of the code, and the operating system holds them. Any code may run inside;
none of it reaches past them.

**A sandbox** is a resource an admin binds: the tools inside (a root filesystem or an image,
read-only), and its limits.

```
holon sandbox add recon --image ./recon-tools.tar --cpu 1 --memory 512M --time 5m
```

A flow names `sandbox recon`, and the walls are made of what Holon already grants it:

- **Files.** Only the flow's bound folders, mounted read-only or read-write as its grant says
  (`FileRead`, `FileWrite`). Nothing else of the host is there: no server data, no secrets key,
  no home folder.
- **Network.** No interface but loopback, where a proxy the host runs is the only way out: it
  connects only to hosts the flow is granted (`HttpRequest`), refuses private addresses as
  `outbound.rs` does, and logs each connection as an outbound round.
- **Secrets.** None, unless the call names some, passed in as environment variables. The taint
  rule can't follow a value inside the sandbox, so everything that comes out of a run's sandbox
  that was given a secret counts as computed from it (§8, fork 14).
- **Limits.** CPU, memory, processes, wall time, output bytes and the scratch disk, with defaults
  and ceilings in the policy.

The sandbox's binding never widens the flow's grant: the walls are where the two meet. The item
lists them:

```
it may:
  run code in the sandbox "recon" (tools: recon-tools)
    reading folder "statements", writing folder "reports"
    connecting to api.bank.example
    up to 1 CPU, 512 MiB, 5 minutes
```

**In the language.** `recon.run(script)` runs a script in a fresh sandbox and answers
`{ status, stdout, stderr }`; `recon.session()` opens a shell to `send` and `expect` as a
terminal is (§5.7), for the run's length. A fresh sandbox per run is the default; a **persistent**
one, its scratch disk kept between runs and held by one run at a time, is an admin's option.

**What lands, and what the log holds:**

- **Writes land when the run commits.** A read-write folder is mounted over a copy-on-write layer.
  When the run finishes, the files it changed are written into the folder as the run's
  `FileWrite` effects, under the folder's commit lock, cap and create-only rules; a failed or
  discarded run leaves the folder as it was.
- **What went in and came out is the run's input:** the script, what was sent and answered,
  stdout and stderr, the hashes of the files the mounts held when it started, and each
  connection. A replay and `holon verify` use that; they don't run the sandbox again.
- **What can't be checked, said so.** Through the proxy, an HTTPS connection's contents are
  encrypted, so the log holds its host, its times and its size, not its requests: weaker than an
  `http.post` round, whose request and answer the log keeps (§8, fork 15). Which files inside a
  mount the code read isn't known either, only what the mounts held. A run's report says both.

**What holds the walls.** Each platform is offered only with walls its operating system
enforces:

| Platform | Walls | Offered |
|---|---|---|
| Linux | user, mount, PID and network namespaces; seccomp; Landlock; cgroups v2 for the limits (as bubblewrap does), no daemon | yes, first |
| any, through the operator's container runtime (Docker, Podman, Kubernetes) | the runtime's own: on Windows and macOS, a Linux VM | yes, where the admin points the server at one |
| Linux, a micro-VM (Firecracker, gVisor) | the strongest | later, if a job needs it |
| Windows, alone | job objects limit resources but don't isolate files or network; AppContainer doesn't fit shell tools | **no** |
| macOS, alone | its sandbox profiles are deprecated | **no** |

A server that can't hold the walls refuses to bind a sandbox, and says why. **WebAssembly with
WASI** is the exception that runs everywhere: the bound folders are its only files, and it is
deterministic, so a run could be run again exactly. Holon already runs WebAssembly modules
(`holon-module`, wasmi, with fuel and memory budgets and no imports). But only tools compiled to
WASI run in it, not `bash` or a bank's client, so it is a later backend for scripts that must
replay exactly (§8, fork 16).

**Who it's for.** Agents first. An agent gets a real shell, in a box whose walls are its grant,
with everything it typed, ran, wrote and connected to in the log: the governance Holon claims,
for the tool agents ask for most.

### 5.9 Outbound streams and long-lived connections

**What is covered.** A streamed answer inside one run: `http_stream` (rc.23) forwards an
upstream's body to the route's caller as it arrives, or with `pipe` to a channel's subscribers,
and the flow gets the whole answer at the end, logged, so the run replays. A provider that only
answers "ask again" is a clock handler and `http.get` (R2). Streams out to callers are channels:
server-sent events, resumed by `Last-Event-ID`.

**What isn't** is every connection that **outlives a run**: a feed the host listens to, a session
that runs send messages over, a payment network's socket. They share one rule:

- **the host owns the connection** (opening, reconnecting, keeping it alive);
- **a run sees whole messages**, never bytes off the wire;
- **what a run sent and received is its input**, logged, so a replay and `holon verify` need no
  connection;
- **the connection's own life** (sign-ons, heartbeats, messages no run took) is outside every run;
  it has its own record (§8, fork 20), and a run's report never claims it.

Three shapes.

**A feed: messages that start runs.** An admin binds a feed: an SSE or WebSocket URL, a gRPC
server stream, a Postgres `LISTEN` channel on a bound database, an MQTT topic; credentials from a
secret source.

```holon
feed prices

on event prices (e) {
  …
}
```

- **The item names the endpoint,** not only the host: `Subscribe("prices")`, shown as "run on
  each event from wss://feed.bank.example/v1/prices". A feed is a trigger (R3d); it sits beside
  routes, clocks, files and actions in a flow with several handlers (triggers L4).
- **One run per message,** or per batch where the binding says (`batch: 100, within: 1s`).
- **Where it resumes.** The host keeps the feed's position (an SSE event id, or a sequence number
  the binding names the field of) in the server's data, and moves it past a message only once the
  run for it has committed. A restart resumes after the last committed message: at least once,
  and the message's id, kept with the claims (as a queue's is, §5.5), makes it once.
- **Order.** Runs start in the order messages arrive; an `ordered` binding runs them one at a
  time, so they commit in that order too.
- **When runs are slower than the feed.** A bounded buffer (`feed_buffer`, a limit); when it is
  full the host stops reading, and the sender waits, as TCP makes it. A feed that can't wait (a
  market feed drops a slow reader) is bound `on_full: skip`, and the next run hears how many it
  missed (`e.missed`), as a channel's subscriber hears `event: missed`.
- **Its state shows** on the deployment's page: connected, behind by *n*, reconnecting since.
  A paused or stopped deployment keeps its position and stops reading.

**A session: send, and wait for the answer.** A WebSocket protocol where a run sends a message
and waits for its reply (a realtime API) is a terminal (§5.7) with a WebSocket backend:
`ws.send(msg)`, then `ws.expect(match, timeout: 5s)` answers the first message that matches,
JSON in and out as values. Held for one run at a time, or across runs as a terminal is.

**A socket: payment networks.** ISO 8583 (card authorisation, with a processor), FIX (trading),
SMPP (an SMS gateway) and many banks' own links are long-lived TCP sessions, with framing, a
sign-on and heartbeats. An admin binds a **socket**:

```
holon socket add processor tls://10.0.4.12:5000 --identity processor --frame length:2:be \
  --sign-on ./signon.msg --heartbeat ./echo.msg --every 60s --match 11,41
```

- **The host keeps the session:** TLS with a client identity (§5.4), the framing (a length prefix
  of *n* bytes, or a delimiter), the sign-on, and the heartbeats (ISO 8583's 0800 echo, FIX's
  Heartbeat), answered while no run is there, from templates fixed in the binding: nothing a run
  computes (§8, fork 18). Sequence numbers (FIX's `MsgSeqNum`) and resend requests are the
  host's too, kept across restarts.
- **Runs share it by correlation, not by a lock.** A run sends a message and waits for the reply
  whose fields match its own (the binding names them: ISO 8583's STAN and terminal id, fields 11
  and 41; FIX's `ClOrdID`), so many runs use one session at once.
- **What no run waits for starts one:** a reversal, an execution report, an unmatched answer, as
  `on message processor (m) { … }`.
- **Messages are bytes the run builds and reads with leaves** (R1): ISO 8583's bitmaps and FIX's
  `tag=value` are formats, like CSV and XML (§5.1). A message is a bytes value (§5.10).
- **The item:** `SocketUse("processor")`, shown as "exchange messages with processor (tls
  10.0.4.12:5000)", and the `on message` trigger. Such links sit on private networks, so a socket
  an admin binds may reach a private address; a flow's own text never can (§8, fork 19).

**Inbound WebSockets** (§5.12, step 1.9): a browser connects to the flow's own port, and each
message it sends starts a run, by the same four rules. Until
2026-11-04 they waited for a job ("channels (SSE) and a POST cover what a browser sends and
hears"). Decided then that they are fundamental: the libraries web apps are built on expect one
two-way connection. The statement is `websocket`, since *socket* is the bound connection above.

**What a trading bot adds** (2026-11-06). A bot that trades on a market feed is the feed's
hardest job, and the shapes above hold it: a feed for prices, a socket for FIX, `http.post` for
an exchange's REST orders, signed with `hmac_sha256` and `hex`, which core has. Four things the
shapes don't yet say:

- **A feed's handler has a log level.** §5.9 was written before rc.32's `lazy` and `unrecorded`;
  they go on `on event` as on any handler (`on event prices (e) lazy { … }`). A strict run per
  message is paced by the disk's sync: about 2 ms on a desktop SSD, so 1,600 strict runs a second
  at 8 connections on one Windows machine
  ([performance](../engineering/themes/performance.md#limits)), short of a busy market feed. The
  position then follows the record: it moves past a message once that run's record is on disk
  (strict: at its end; lazy: at the next sync); an unrecorded run's message moves it when the run
  ends. So a crash delivers again what wasn't on disk, a lazy handler's last second, and those
  runs run again: their claims were in the records lost. A handler whose run has an effect that
  mustn't repeat (an order) stays strict, or makes the effect safe to repeat (below); the prices
  it reads can be lazy (fork 25).
- **The newest price, not the queue.** `on_full: skip` drops what arrives while the buffer is
  full and keeps what is queued: for prices, the stale ones. A market feed wants **conflation**:
  `on_full: latest, key: symbol` keeps one waiting message per key, the newest, and the run hears
  how many it replaced (`e.replaced`), as `e.missed` says skipped ones (fork 26).
- **Periods under a second.** Clocks take whole seconds and are checked once a second
  ([triggers](triggers-L4.md)), so a bot can't poll a price, or give up on an unfilled order,
  faster than that. A change to the scheduler's one-second pass, not a host capability (fork 27).
- **An order out before its record.** A run's requests go out during its rounds and its entry is
  written after (`firing.rs`: the rounds, then `log_run`'s `sync_through`), so a crash between an
  exchange accepting an order and the entry's sync leaves an order no record shows. G1 (an
  answered strict run is on disk) holds; it never covered what a run did before its answer. The
  exchange's acceptance is outside Holon, so the flow makes it safe, and step 1.6's sample shows
  how: a client order id derived from what decided the order (the signal's id, the candle's
  time), never from the run's nonce, since a retried request is a new run with a new one, so the
  exchange refuses the second; and on start a reconciliation against the exchange's open orders.
  A socket's FIX correlates by `ClOrdID` the same way. The same holds for any call with an effect
  (a payment, a message sent); `refund-once`'s claim covers two runs at once, not a crash between
  a call and its commit.

**Engines.** The data plane already runs on tokio, so a WebSocket client is tokio-tungstenite on
that runtime, and inbound WebSockets' server side is the same crate. SSE is a streaming read the
outbound client can do. A Postgres `LISTEN` is the
`postgres` crate's notifications, beside `engines.rs`. MQTT is rumqttc. gRPC (tonic, HTTP/2 and
protobuf) waits for a job that brings one. A socket's framing and session are Holon's own code,
small; the protocols' meaning stays in leaves.

#### 5.9.1 Step 1.6 as designed (2026-11-09, against `426b659`): feeds and a queue

§5.5 and the text above are in the language as it was. In TypeScript a feed and a queue are
imported as a mail is, a queue sends with a method as a mail does, and each is listened to as a
mail is:

```ts
import prices from "feed:prices"
import payouts from "queue:payouts"

on.feed(prices, (e) => { … }, { log: "lazy" })
payouts.send({ id, amount })
on.queue(payouts, (m) => { … })
```

`on.feed` and `on.queue`, not `on event` and `on message`: a handler is named for what it
listens to, as `on.mail` and `on.file` are, and *message* stays the socket's (fork 61).

Six slices, each with its sample:

| Slice | What | Sample |
|---|---|---|
| 1.6a | the subscription runtime, and server-sent events: `feed:` bindings, `on.feed`, the position (`Last-Event-ID`), once per message, a bounded buffer that stops reading when full, reconnecting, the feed's state on the deployment's page | `price-feed` |
| 1.6b | WebSocket feeds: a position the binding names the field of, a subscribe message and a heartbeat from the binding, `--on-full skip` and `latest` (forks 17, 26), a lazy or unrecorded handler's position (fork 25) | `trading-bot` |
| 1.6c | a WebSocket feed resumed from its position, where the binding says the feed takes it (`--resume`, fork 64) | `bluesky-mentions` |
| 1.6d | a feed's gaps: the numbers a feed that can't replay skipped, on each event (`--sequence`, `e.gap`) | `trade-tape` |
| 1.6e | the queue, SQS's API (fork 8): `queue.send` once the run's writes land, `on.queue` deleting each message once its run is over, AWS's request signing in the host | `payout-events` |
| 1.6f | a Postgres `LISTEN` channel on a bound database, as a feed | `ledger-watch` |

Each slice gets its own "as designed" before its code, as 1.3's did. What follows is what they
share.

**The binding.** A feed's URL is kept where a server's is, in a secret source, since a feed's
token often rides in it:

```
HOLON_SECRET_PRICES_URL=https://stream.example.com/v1/prices?token=…
holon feed add prices --url env:HOLON_SECRET_PRICES_URL [--header NAME=SOURCE]…
                      [--buffer N] [--on-full wait|skip|latest [--key FIELD]] [--idle T] [--for SLUG]
```

- The scheme says which kind: `https://` server-sent events, `wss://` a WebSocket (1.6b);
  `http://` and `ws://` only to this machine, as mail's plain servers. A feed may reach a private
  address, since it is the admin's to bind and the binding shows where (fork 19); a flow's own
  text still can't.
- `--header` adds a request header whose value is read from a source
  (`Authorization=env:HOLON_SECRET_PRICES_AUTH`).
- Checked when it is bound: connected to, its answer's status and content type read
  (`text/event-stream`), and closed. The URL is never stored, shown or logged: the binding keeps
  the source, and where it points without its login or query.
- Several deployments may listen to one feed, each on its own connection with its own position: a
  feed is a broadcast. A queue's messages are shared out among those who read them, so one
  deployment listens to a queue, as one listens to a mailbox.

**The runtime.** For each feed a running deployment listens to, the host keeps a connection (a
thread reading it) and a runner (a thread starting its runs), with a bounded buffer between them
(`--buffer`, 1,000 messages unless given).

- **A run per message,** one at a time per feed, in the order they came. One at a time keeps the
  feed's order and makes the position one id, the last message whose run is over. A strict run is
  paced by the disk's sync (about 500 a second), a lazy one by the run itself (fork 58). Batches
  (`batch: 100` above) wait for a job.
- **The position.** What the feed gives as each message's id (an SSE `id:`, a field the binding
  names, a queue's message id) is kept in `feed-seen/<slug>/<feed>.json`, as a mailbox's seen UIDs
  are, and moved past a message once its run's record is on disk: a strict run's at its end, a
  lazy one's at the next sync, an unrecorded one's at its end (fork 25). A reconnect resumes after
  it (`Last-Event-ID`). A feed that gives no ids resumes where it is, and the binding says so.
- **Once per message:** the last 1,000 ids run on are kept beside the position, and a message
  among them is passed over without a run, on the timeline, so a feed that resends after a
  reconnect runs nothing twice. A crash between a lazy run and its sync runs it again (fork 25).
- **When the buffer is full** the connection stops reading, and the sender waits, as TCP makes it
  (`--on-full wait`, the default, fork 17). `skip` drops what comes while it is full, and the next
  run hears how many (`e.missed`); `latest` keeps one waiting message per value of `--key`, the
  newest, and its run hears how many it replaced (`e.replaced`; 1.6b, fork 26).
- **Reconnecting:** a connection lost, refused, or silent for `--idle` (90 s unless given; a
  feed's own heartbeats keep it alive) is opened again after a second, doubling to a minute. The
  timeline says so once, and again when it connects.
- **Its state** is in the deployment's API and on its server panel: connected since, *n* waiting,
  or reconnecting since, with the last error. A paused or removed deployment closes its
  connections and keeps its position; a new version opens them again from it.
- **What the connection did outside any run** (connects, reconnects, skipped messages) is on the
  deployment's timeline, as a mailbox's looks are. The record of its own that fork 20 gives a
  connection waits for the socket, whose sign-ons and heartbeats need it (fork 59).

**The run** is the handler's alone, as a mail handler's is: `kind: "feed"` or `"queue"`, its label
the handler's, the message sourced (`FeedEvent("prices")`, `QueueMessage("payouts")`) with the
clock and the random bytes, so a replay and `holon verify` need no connection. A parked run keeps
the message among its sourced values.

`e` is the message:

| Field | |
|---|---|
| `data` | a value when its data is JSON, else its text; bytes for a binary WebSocket frame |
| `id` | its id, when the feed gives one |
| `event` | an SSE event's name, `message` unless it says |
| `missed`, `replaced` | how many were skipped, or replaced, before it (`--on-full`) |

**The queue (1.6e).** SQS's API, which AWS's queues speak and so do services that copy it
(ElasticMQ, LocalStack, Yandex Message Queue). The URL holds the access key as its login, as
mail's URLs hold theirs: `https://<key id>:<secret>@sqs.eu-west-1.amazonaws.com/123456789012/payouts`,
the region read from the host, or from `?region=` for another service. Requests are signed with
AWS's Signature Version 4, written in the host (`sigv4.rs`, on the `hmac` and `sha2` core already
uses), so step 1.4's S3, when it comes, signs with it too.

- `payouts.send(data, { group })` sends once the run's writes land, as mail does, kept and tried
  again through an outbox like mail's. `group` is a FIFO queue's message group; its deduplication
  id is the run's and the send's, so a retry is one message.
- `on.queue(payouts, (m) => …)` long-polls (20 s), runs each message one at a time, and deletes it
  once its run's record is written, a failed run's too (its record says why), as a mail is moved
  (fork 60). A run still going when the message's visibility runs out (`--visibility`, 5 minutes
  unless given) extends it, so the message doesn't come back while it runs.
- `m` is the message: `data` (a value when it is JSON, else its text), `id`, `attributes` (a
  record of text), `sent` (seconds since 1970, as `now` is) and `count` (how many times the queue
  has handed it out).

**The grant.** `FeedEvent("prices")`, a driver, shown as "run on each event from prices
(wss://stream.example.com/v1/prices)"; `QueueMessage("payouts")`, "run on each message from
payouts, and delete it once its run is over"; `QueueSend("payouts")`, a sink, "send messages to
payouts". A message computed from a secret is refused, as a channel's pack is.

**Engines.** Server-sent events are a streaming read through `ureq`, as the token request is.
WebSocket is `tungstenite` (1.6b): the one new dependency, and the one step 1.9's server side
uses, through `tokio-tungstenite`. SQS is `ureq` and `sigv4.rs`. `LISTEN` is the `postgres`
crate's notifications, on a connection of its own to the bound database.

**Stand-ins.** The server tests run each against one in memory, as the IMAP one is: an SSE
server, a WebSocket server, and an SQS that checks each request's signature. The e2e server starts
the same in Node. 1.6f's e2e test needs a Postgres beside the e2e job's MongoDB.

#### 5.9.2 Slice 1.6a as designed (2026-11-09, against `426b659`): server-sent events

The runtime of §5.9.1, with server-sent events as its first kind of feed.

**The language.** `import prices from "feed:prices"` imports a feed as a mail is imported;
`on.feed(prices, (e) => …, { log })` is its handler, refused when its first argument isn't an
imported feed, when a feed has two handlers, and at a `return` with a value (an event's run
answers nobody). Its node is `on_feed` (`declare = "on_feed"`, `feed`), its leaf `feed_event`
(`feed`, sourced as `mail_arrival` is), its label `on feed prices`. `holon.d.ts` gives `e` a
`FeedEvent` type: `data`, `id` and `event` in this slice (`missed` and `replaced` come with
`--on-full`, 1.6b).

**The grant.** `FeedEvent("prices")`, a driver, shown as "run on each event from the feed
prices". A deploy that listens to a feed no one bound is refused, as one that reads an unbound
mailbox is.

**The binding** (`feeds.rs`, beside `mails.rs`, its file `feeds.json`):

```
holon feed add prices --url env:HOLON_SECRET_PRICES_URL [--header NAME=SOURCE]… [--buffer N] [--idle T] [--for SLUG]
holon feed list
holon feed remove prices [--for SLUG]
```

and `GET`, `POST` and `DELETE` on `/admin/feeds`, and the admin page's resources. The URL is
`https://` (plain `http://` only to this machine); a header's value is read from its source when
the connection opens, never kept. Bound only once a request to it answers 200 with
`text/event-stream`, its connection then closed. The binding keeps the sources, and where the URL
points without its login or query (`https://stream.example.com/v1/prices`), which is what is
shown.

**Server-sent events** (`sse.rs`): a `GET` with `Accept: text/event-stream`, through `ureq`, its
body read as it comes. Lines are read as the standard says: `data:` lines joined with a newline,
`id:` (one holding a NUL ignored), `event:`, `retry:` (the feed's own wait before reconnecting,
taken when it is longer than ours), comments (`:`) and blank lines; an event is dispatched at a
blank line when it has data. One event is at most 1 MiB, a line too; past that the connection is
closed and opened again, on the timeline. The reconnect sends `Last-Event-ID`.

**The runtime** (`subscriptions.rs`, for every kind of feed to come): every second a thread
compares the feeds running deployments listen to with the connections it holds, and opens or
closes them; each has a reading thread, a bounded buffer and a running thread, as §5.9.1 says. A
new version of a deployment closes the old connection and opens one from the position. The
position and the last 1,000 ids are in `feed-seen/<slug>/<feed>.json`, with where the feed
points: a binding pointed somewhere else starts anew, as a mailbox's seen UIDs do.

- **Where a new listener starts:** at what the feed sends from now. A feed that replays its
  history to a first connection without `Last-Event-ID` runs it, since the host can't tell
  history from news.
- **The position moves at the run's end** in this slice, for every log level; 1.6b makes a lazy
  handler's follow its sync (fork 25).
- **What the timeline says:** `listens to the feed prices (https://…)`, `lost the feed prices:
  <why>; trying again in 4s` once, `reads the feed prices again`, and `passed over an event of
  prices it ran already (41)`.

**The run:** `kind: "feed"`, its label `on feed prices`, `e` sourced as `FeedEvent("prices")`
with the clock and the random bytes; `holon verify` reruns it as that handler alone, and a parked
one keeps `e`.

**Its state** is in `GET /deployments/<slug>/feeds`: each feed's state (`connected` since,
`reconnecting` since, with the last error), its position, and how many wait. The editor's server
panel lists feed handlers with it; a run is titled `event from prices: <its event's name> (on
feed prices)`; the palette has `feed_event`.

**The sample, `price-feed`:** a feed of prices; each event's price is kept per symbol in state,
sent on a channel its page shows as they come, and answered at `GET /prices`. Run live and in the
e2e tests against a stand-in: an SSE server whose events are files dropped in a folder, as the IMAP
stand-in's messages are, kept so a reconnect with `Last-Event-ID` is answered from after it.

**Tests:** the parser (data lines, ids, a NUL in an id, `retry:`, comments, CRLF and CR, an event
too long); the binding refused for a bad source, a URL that isn't `https://`, an answer that isn't
`text/event-stream`, and a header source that isn't there; the server test binds a feed to an SSE
stand-in in memory, deploys the sample, sends events, drops the connection and sees the reconnect
resume with `Last-Event-ID`, a resent event passed over, and the runs verify.

#### 5.9.3 Slice 1.6a as built (2026-11-09)

As designed, with what building it found:

- **The client is the host's own** (`sse.rs`), over the `rustls` and `webpki-roots` the mail and
  the databases use, not `ureq`: a deployment stopped or a binding removed closes its connection
  at once from the supervisor's thread (a clone of the socket, shut down), and `--idle` is the
  socket's read timeout. On Windows a cloned socket's timeout is its own, so it is set on the one
  the read uses. The body is chunked, of a length, or up to the close. A redirect isn't followed:
  the headers it would carry are the binding's, and a binding to one says it answered a redirect.
  A login in the URL is sent as Basic authentication; a header's name is checked, and a value
  with a line break refused.
- **An event past 1 MiB is passed over,** not a reason to close. Closed and opened again, as
  designed, a feed that keeps its events sends the same one after `Last-Event-ID`, for ever. Its
  lines are dropped as they come (a line is kept to a byte past 1 MiB), the timeline says `passed
  over an event of prices past 1 MiB`, and the rest is read on.
- **Read, and run.** The reconnect sends the id of the last event read: those in the buffer still
  run, from the same thread. What is kept on disk is the last run on, so a server started again
  opens from there. Only an event with an id of its own is passed over as sent again; one without
  can't be told from another.
- **No event runs twice across versions:** a run lock per deployment and feed outlives a
  connection while a run of the old one holds it, so a new version's running thread waits for that
  run, and reads where the feed is under the lock before each event.
- **The timeline,** as designed, but `lost the feed prices: …; trying again in 1s` is said only
  when a connection can't be opened, once until `reads the feed prices again`: a connection that
  drops and opens again at once is in the feed's state, not on the timeline, which a feed whose
  server ends its connections every few minutes, as proxies do, would fill.
- **Its state** is also `closed`: no connection (the deployment stopped, or not yet opened). The
  server panel asks for it every 3 seconds (`connected, at 41`, `reconnecting: <why>`, `, 3
  waiting`). The admin page's resources bind a feed, its headers a `NAME=SOURCE` a line, and show
  it as `<address> (from <source>) · 1000 may wait · quiet for 90s at most`; permissions list it as
  a resource, price-feed "starts on each event it sends".
- **The sample** keeps each price in a collection, `latest`, one item a symbol, which answers
  `GET /prices` and draws the page; not the state. It doesn't keep `e.id`: an event without one
  would leave the write without a value, and nothing would be kept.
- **The tests:** the parser (data lines, ids and a NUL, CRLF, LF, CR and a byte-order mark,
  `retry:`, an event and a line past 1 MiB passed over and the rest read), the URL (`https://`,
  `http://` to loopback only, shown without its login or query), a connection read, closed from
  another thread and opened again with `Last-Event-ID`, an idle feed found out, and an answer that
  isn't a feed refused, against an SSE stand-in in memory (`sse::stand_in`, whose `/events` is
  chunked and can be cut); the binding (refused for a value that isn't a source, a source that
  isn't there, a remote `http://`, an answer of `text/plain` or 404, a header's value in clear, an
  idle time or a buffer out of bounds, a name; kept without the query) and a deploy refused while
  unbound. The server test binds `prices` to the stand-in with a key in the URL's query, deploys
  the sample, sends three events (three runs, the prices, `connected` at 3), cuts the connection
  (opened again with `Last-Event-ID: 3`), sends 3 again (passed over), one past 1 MiB (passed
  over, the connection read on) and 4 (a fourth run), verifies the runs, finds no key in the log,
  and stops it (closed, nothing more runs). The e2e server starts a stand-in whose events are files
  dropped in a folder (`editor/e2e/sse-server.mjs`) and binds `prices`; the test deploys the
  sample in the editor, drops an event, reads the price and the server panel's line, cuts the
  connections, sees the feed opened again from the event run on, drops another, and reads the
  binding on the admin page and the grant on permissions.
- **Run live** on a scratch server against the e2e's stand-in: four events, four runs; a cut,
  opened again from `e003`; the feed stopped, `lost the feed prices: … can't be reached …; trying
  again in 1s` and the state `reconnecting`, the position kept, the flow still answering.

#### 5.9.4 Slice 1.6b as designed (2026-11-09, against `22a3b56`): WebSocket feeds

WebSocket feeds on 1.6a's runtime, what a full buffer does besides waiting, and the position of a
handler that isn't strict.

**The binding.** A `wss://` URL is a WebSocket feed (`ws://` only to this machine), with 1.6a's
options and these:

```
holon feed add prices --url env:HOLON_SECRET_PRICES_URL
                      [--subscribe MESSAGE]… [--heartbeat MESSAGE [--every T]]
                      [--id FIELD[,FIELD]…] [--only FIELD=VALUE]…
                      [--on-full wait|skip|latest [--key FIELD]]
```

- **`--subscribe`** is a text message sent each time the connection opens, in order, again for
  another: the subscription most market feeds want
  (`{"type":"subscribe","channels":["ticker"],"product_ids":["BTC-USD"]}`). One that starts
  `env:` or `vault:` is read from that source, for a feed whose sign-in is a message with a key in
  it; the binding keeps the source, and a written one as written, which is what is shown.
- **`--heartbeat`** is a text message sent every `--every` (30 s unless given) while the
  connection is open, for a feed that drops a client that says nothing (`ping`). WebSocket's own
  pings are answered by the host, always. Neither starts a run or is on the timeline: what the host
  says for the deployment comes from the binding, never from a run (fork 18).
- **`--id`** names the field of a JSON message that is its id: a dotted path (`data.u`), or
  several joined with `/`, for a feed whose sequence is per symbol (`--id product_id,sequence`
  gives `BTC-USD/41`). Without it a WebSocket's messages have none.
- **`--only`** runs only the messages whose field has one of the values given (`--only
  type=ticker`, again for another value or field; every field named must match). The rest, the
  answers to the subscription and the heartbeat, are passed over without a run, counted in the
  feed's state, not on the timeline (fork 63).
- **`--on-full`** and **`--key`**, below, are any feed's. `--subscribe`, `--heartbeat`, `--id`
  and `--only` are a WebSocket's; an SSE binding that gives one is refused, since an SSE event
  has its own id.
- **Checked when bound:** the handshake answers 101, then the connection is closed. A redirect
  isn't followed, as for SSE.

**The connection** (`ws.rs`, beside `sse.rs`, sharing its URL, socket and TLS): the handshake
through `tungstenite`, the one new dependency, with its `handshake` feature and no TLS of its own,
reading the host's own stream, so a connection is closed from another thread and found quiet by
its read timeout as an SSE one is; the binding's headers, and a login in the URL as Basic
authentication. A text frame is a message, its data a value when it is JSON, else its text; a
binary frame's data is bytes. A ping is answered with its pong; a close frame ends the connection,
which opens again as any lost one does, and sends its subscription again. A message past 1 MiB
ends the connection, on the timeline: unlike an SSE event, a WebSocket message can't be passed
over without reading it, and since a reconnect doesn't resume (below), the feed doesn't send it
again.

**The position** is the last id run on, as for SSE, and the last 1,000 ids pass over a message
sent again (the snapshot a feed sends after a reconnect, say). It isn't sent back: a WebSocket has
no `Last-Event-ID`, and the feeds a job brings send a snapshot on subscribe, not a replay
(fork 64). A new listener, and a reconnect, start at what the feed sends from then.

**When the buffer is full** (`--on-full`, forks 17 and 26):

- **`wait`,** unless given: the connection stops reading, as in 1.6a.
- **`skip`:** what comes while the buffer is full is dropped, and the next message that finds
  room carries how many were dropped before it, `e.missed`. A dropped message's id isn't kept: it
  never ran.
- **`latest --key product_id`:** a message replaces the one of its key that waits, in that one's
  place, full or not (fork 62), and its `e.replaced` is how many it replaced, theirs counted with
  it. A message without the key waits as it came. The buffer holds `--buffer` messages, so a key
  that finds it full of others waits for room, as with `wait`. `--key` is a dotted path, as
  `--id`; `latest` without it is refused.

`e.missed` and `e.replaced` are on every event, 0 unless the binding skips or replaces, so a flow
reads them whatever its binding says (an absent field would stop what reads it); SSE's events
gain them too.

**A handler that isn't strict** (fork 25). In 1.6a the position moved at every run's end. Now it
follows the record: a strict run's at its end, its record on disk, as now; a lazy run's once the
sync that covers its record is done, held as its channel packs and mail are and released by
`sync_logs` beside them; an unrecorded run's at its end. Until then it is kept in memory, so the
connection, and a new version's, go on from it. A crash loses the lazy runs the last sync didn't
cover, and their events run again, their claims gone with their records. The deploy says so where
it lists the handler's level: "on feed prices is lazy: after a crash, the events of its last
second may run again"; `Unrecorded("on feed prices")` says the same. `holon check` lists a mail
and a feed handler's level as it does a route's, which 1.3b and 1.6a left out.

**Its state** adds how many were passed over by `--only`, missed and replaced since the
connection opened: the server panel says `connected, at BTC-USD/41, 3 waiting, 120 replaced`.

**The run** is 1.6a's: `kind: "feed"`, `e` sourced as `FeedEvent("prices")`, a binary message's
bytes logged as any bytes value is.

**The sample, `trading-bot`:** a market's ticker over a WebSocket, shaped as Coinbase Exchange's
public feed is (`product_id`, `price` as a decimal in text, `sequence`), and orders to an exchange
over HTTP, signed with `hmac_sha256`. It shows §5.9's four points:

- the prices, conflated by the binding (`--on-full latest --key product_id`) and handled `lazy`:
  each symbol's newest price kept, with how many it replaced;
- the order strict: a price at or under a symbol's level, which a route sets, starts the flow's
  own `order` action (a start, not awaited), which places it;
- the order's client id the level's own, set when the level was, never the run's: a lazy run a
  crash ran again, or the next price under the level, sends the same id, and the exchange refuses
  it the second time;
- an order over a limit waiting for approval (`await event("approved")`, as `approval` does), and
  an action that reads the exchange's open orders back after a restart, run by hand, since
  nothing starts a run at start.

Its text takes what the language has now: one order a run, and no loop that waits on a call
(the TypeScript slices 6 to 11, after Tier 1); fork 65 says what that leaves out.

**Stand-ins.** The server test's WebSocket feed is `tungstenite`'s server side in a thread (it
records what each connection was sent, and can be cut), and the exchange an HTTP stand-in that
checks the signature and refuses a client order id twice (409). The e2e server starts the same in
Node: `editor/e2e/ws-server.mjs`, its messages files dropped in a folder as `sse-server.mjs`'s
are, written by hand (the handshake and text frames), since Node has no WebSocket server of its
own; the exchange in the test.

**The live run:** the prices from Coinbase Exchange's public feed
(`wss://ws-feed.exchange.coinbase.com`, the `ticker` channel, which needs no account), bound with
`--subscribe`, `--only type=ticker`, `--id product_id,sequence` and `--on-full latest --key
product_id`; the orders to the stand-in exchange (fork 66).

**Tests:**

- **`ws.rs`:** the handshake and its headers, Basic authentication, text and binary frames, a
  ping answered, a close and a cut, a message past 1 MiB, idle found out, a heartbeat sent when
  due, and a subscription sent on each connection.
- **The buffer:** `skip`'s count, `latest`'s replacement in place with its count, a key that
  waits for room, and a message without the key.
- **The position:** a lazy run's moved only by the sync that covers it, and an unrecorded one's at
  its end.
- **The binding:** `--id`, `--only` and `--key` paths; WebSocket options refused on an SSE
  binding, and `latest` without a key.
- **The server test** binds `prices` to the stand-in, deploys the sample, sets a level, and sends
  prices: they are conflated while a run is slow, the order is placed once, it isn't placed again
  when the price comes again, it is refused by the exchange when a run is made again by hand, and
  it waits for approval over the limit. It also checks the runs verify, and that the key is in no
  log.
- **The e2e test** deploys it in the editor against `ws-server.mjs` and the exchange stand-in,
  sets a level, drops prices, reads the order and the server panel's line, and approves a large
  order.

#### 5.9.5 Slice 1.6b as built (2026-11-09)

As designed, with what building it found:

- **A message past 1 MiB is passed over,** as an SSE event is, not a reason to close: `tungstenite`
  reads a message whole before handing it over, so the host reads up to 16 MiB, passes over what
  is past 1 MiB (on the timeline), and reads on. Only past 16 MiB does the connection end and open
  again. Ending it at 1 MiB, as designed, would have lost what came after it, and the feed doesn't
  send it again.
- **A message sent again writes nothing.** It is passed over by its id, as designed, and where the
  feed is moves with the next run's, not on its own: a lazy handler's position then moves only
  with a record that a sync covers.
- **`holon check` says it:** `on feed ticker is lazy: after a crash, the events of its last second
  may run again`, after what the flow holds while it runs. Its unrecorded lines now take mail and
  feed handlers too, which 1.3b and 1.6a left out.
- **The sample, as the language has it:**
  - The order's action is `place`. The handler marks the level `started` when a price comes under
    it, and reads nothing `place` writes: a run that starts another holds the keys it read until it
    commits (its claims), so `place`, writing `orders`, would wait on a handler that read
    them. The mark is also what keeps the next price under the level from sending the order again.
  - A call bound on a statement is made when the run reaches it, whatever a plain `if` around it
    says: only an absent input keeps it from being made. So the handler writes
    `place(!large ? order : undefined)` and `approval(large ? order : undefined)`, and `order` is
    absent unless the price is under the level.
  - The approval is an action of its own, `approval`, which waits for the event and then starts
    `place`: a resumed run keeps the clock it began with, so an order placed from it an hour
    later would be signed with an hour-old time, which an exchange refuses. `place` signs with the
    time its own run began.
  - `reconcile`, run by hand, answers the exchange's orders as one value.
- **Its state** counts per connection, as designed: `connected, at BTC-USD/41, 1 waiting, 2
  replaced, 1 passed over` (the subscription's answer).
- **`ring`** is a dev-dependency of `holon`: the server test's exchange checks the signature with
  it. It was built already, under `rustls`.
- **The tests:**
  - `ws.rs`: a connection opened with its headers, Basic authentication and subscription, read as
    it comes (text, binary, a message past 1 MiB passed over and the rest read), cut, and closed
    from another thread; heartbeats sent while it waits and a quiet feed found out; a redirect
    refused. The stand-in (`ws::stand_in`) is `tungstenite`'s server side in a thread.
  - The binding: options read as they say and shown, and refused where they don't fit (a
    WebSocket's on SSE, `latest` without a key, a key without `latest`, `--every` without
    `--heartbeat`); a field found by its dotted path and read as text.
  - The subscription: an event's fields, `skip`'s drop and count, `latest`'s replacement in place
    and a new key waiting for room, and a lazy run's move held until the sync that covers it.
  - The server test binds `ticker` to the stand-in with a key in its query, deploys the sample
    with the exchange's host replaced by an HTTP stand-in that checks the signature, and sets a
    level of 100 x 2. With the exchange holding its answer, prices of 99, 98, 97 and 96 for BTC and
    one for ETH come: 2 replaced, 2 waiting, the subscription's answer passed over. Released, three
    runs: the price kept is 96 with 2 replaced, the order placed once with the level's client id,
    and the position `ETH-USD/1` (BTC's newest kept its place, ahead of ETH). A price of 95 sends
    nothing; `place` run by hand is refused 409. A level worth 2000 waits for approval; the link
    approves it, and a run of its own places it. `reconcile` reads both back, the runs verify, and
    no key or secret is in the log.
  - The e2e test deploys the sample in the editor against `ws-server.mjs` and a Node exchange,
    sets the secrets, reads the subscription in what the feed heard, drops prices while the
    exchange holds, reads the server panel's line (`connected, 1 waiting, 2 replaced, 1 passed
    over`, then `connected, at …/4, …`), the order, the approval of a large one, `reconcile`, and
    the binding on the admin page without its key.
- **Run live** on a scratch server, the ticker bound to `wss://ws-feed.exchange.coinbase.com` (BTC
  and ETH), the orders to a stand-in exchange on loopback with throwaway credentials: the binding's
  check, the handshake; prices kept within seconds, 47 replaced in the first ten and 338 in about
  a minute, the subscription's answer passed over; a BTC level above the market placed one order
  (signed 1 s before it was sent) and none for the prices under it after; an ETH level worth 2500
  waited, was approved 40 s later and placed from its own run (signed 0 s before); `reconcile`
  read both; `place` run again by hand was refused 409. 669 runs exported and verified, and no
  credential in the data or the server's output.

#### 5.9.6 Slices 1.6c and 1.6d as designed (2026-11-09, against `7b6a02e`): a feed resumed, and its gaps

Asked once 1.6b was designed (fork 64): a feed that can send again what came while the host
wasn't reading should be asked to, as SSE is with `Last-Event-ID`; and a feed that numbers its
messages but can't send them again should say, on the next event, how many were lost, so the flow
fetches them. WebSocket feeds come in three kinds (two probed live on 2026-11-09):

- **They replay from a cursor:** Bluesky's Jetstream (`?cursor=<seq>`, the event at the cursor
  sent again, at least once, within a window it keeps), Socket.IO's connection-state recovery,
  GraphQL subscriptions, Nostr's `since`, Kafka and NATS bridges. Each takes the position in a
  place of its own: a query parameter, a header, or the subscription message.
- **They number their messages and can't replay:** Coinbase Exchange's `matches` (`trade_id`, one
  more for each trade of a product; probed: none skipped), `full` and `level2` (`sequence`),
  Binance's depth stream. What is lost is fetched over the provider's REST API, a snapshot or a
  page of what was skipped. That fetch is the provider's, so the flow's; the host says when one is
  needed, and how much.
- **Neither:** a ticker (1.6b's), whose newest value makes the lost ones moot.

**1.6c: `--resume WHERE`,** a WebSocket's (an SSE feed has `Last-Event-ID`), where the feed takes
the position:

- `query:NAME`: the URL opened gets `NAME=<position>`, in place of a parameter of that name in it
  (`--resume query:cursor`);
- `header:NAME`: the request sends `NAME: <position>`;
- `message:TEMPLATE`: sent in place of the `--subscribe` messages, `{position}` in it replaced by
  the position; again for another, in order (fork 68).

It is sent only when there is a position: a new listener opens as the binding is written and reads
from what the feed sends then. The position is the one SSE sends: on a reconnect, the last message
read, since those waiting still run; after a restart or for a new version, the last run on, a lazy
handler's once its record is synced. A feed that sends the position's own message again, as
Jetstream does, has it passed over by the last 1,000 ids. `--resume` needs an `--id` of one field:
a cursor is one value (fork 67). A position older than what the feed keeps is the feed's to answer;
a feed that also numbers its messages one by one says what was lost with 1.6d's `--sequence`.

**1.6d: `--sequence FIELD`,** any feed's whose data is JSON: a whole number that grows by one for
each message, for each `--key` when one is given (`--sequence trade_id --key product_id`). Each
event's `e.gap` is how many numbers were skipped before it: its number, less the last one read of
its key, less one. It is 0 for the next number, for a key's first, for a message whose field isn't
a whole number, and for a number not above the last: a message sent again, or a count begun anew,
as a feed that numbers per connection begins one (fork 71).

- **The last number of each key is kept with the position,** on disk, so a gap across a reconnect
  or a restart is said: what came while the host wasn't reading, which the feed won't send again.
  At most 10,000 keys, the least recently read forgotten, its next message's gap 0 (fork 72).
- **It is the feed's gaps, not the host's:** counted as each message is read, before the buffer,
  so a message `latest` replaced or `skip` dropped is no gap (those are `e.replaced` and
  `e.missed`); a message that replaces another carries that one's gap with its own (fork 69).
- **A message `--only` passes over is counted:** the feed numbered it (a `full` channel numbers
  each message, whatever its type), and a gap it shows is carried by the next of its key that runs
  (fork 70). So what the binding passes over is what the flow doesn't want, not what it can't see.
- **A lazy handler's numbers move with its position,** once synced. After a crash, a feed that
  can't replay doesn't send again the events of its last second; the next message's gap counts
  them, and the flow fetches them with the rest.
- `e.gap` is on every event, 0 unless the binding names a sequence, as `e.missed` and `e.replaced`
  are. `--key` now names the key of `latest` and of `--sequence`; one with neither is refused.
- **Its state** adds the numbers skipped since the connection opened: `connected, at
  BTC-USD/1117440338, 49 in gaps`; `gaps` in `GET /deployments/<slug>/feeds`. Not on the
  timeline: each run's input says its gap.

**The samples** (fork 73):

- **`bluesky-mentions` (1.6c):** Bluesky's posts from Jetstream
  (`wss://jetstream.us-east.bsky.network/xrpc/network.bsky.jetstream.subscribeEvents?collections=app.bsky.feed.post`),
  bound with `--resume query:cursor --id payload.seq --only payload.operation=create`. A lazy
  handler keeps each post that mentions the word a route sets, by its `at://` URI, so a post sent
  again writes the same item, as Jetstream's "at least once" asks. `GET /mentions` lists them.
- **`trade-tape` (1.6d):** Coinbase Exchange's trades (`matches` and the `last_match` it sends
  first, `--id product_id,trade_id --sequence trade_id --key product_id`). A handler adds each
  trade's size to its product's volume; when `e.gap` is above 0 it fetches the trades it skipped
  (`GET /products/<product>/trades?after=<trade_id>&limit=<gap>`, public, 1,000 at most a page)
  and adds theirs (`decimal_sum` of a `map`), so the volume is exact across a reconnect or a
  restart. A gap past 1,000 adds what one page holds and keeps how many it couldn't.

**Stand-ins.** The server test's WebSocket stand-in replays from a `cursor` in its query, numbers
what it sends, and can leave numbers out; the trades' REST API is an HTTP stand-in in the test. The
e2e's `ws-server.mjs` does the same: a dropped file whose `seq` is at or past the cursor is sent
again to a connection that asks from it.

**The live runs** (fork 74):

- `bluesky-mentions` against Jetstream: the server stopped for 30 s and started again; the posts of
  those 30 s run from the cursor, the cursor's own passed over, no `seq` run twice (read from the
  exported runs).
- `trade-tape` against Coinbase Exchange: the server stopped for 30 s and started again; the first
  trade of each product carries the gap, its trades are fetched, and the volume equals the sum of
  the trades REST gives for the same ids.

**Tests:**

- **The binding:** `--resume`'s three places, refused on SSE, without `--id`, or with an id of two
  fields; `--sequence`'s path; `--key` refused with neither `latest` nor `--sequence`.
- **`ws.rs` and the link:** the cursor in the query, the header, the message in place of the
  subscription; none for a new listener; the position read on a reconnect and the one run on after
  a restart; the cursor's own message passed over.
- **The gaps:** the next number, one skipped, several, per key, a key's first, a number not above
  the last, a message without the field, one `--only` passes over carrying its gap to the next, a
  replaced message's gap added to the one that replaced it, the numbers kept across a restart and
  moved with a lazy position, and the 10,000th key forgetting the least recent.
- **Server tests**, one a sample: `bluesky-mentions` cut and restarted, its posts sent again from
  the cursor, each run once; `trade-tape` with numbers left out and the trades fetched, its volume
  exact.
- **e2e tests**, one a sample, in the editor against `ws-server.mjs`.

#### 5.9.7 Slice 1.6c as built (2026-11-09)

As designed, with what building it found:

- **The binding** keeps `resume` as one of `{"query": NAME}`, `{"header": NAME}` or
  `{"message": [MESSAGE, …]}`; `/admin/feeds` and `holon feed add --resume` take each as
  `query:NAME`, `header:NAME` or `message:…`. A written message without `{position}` is refused;
  one read from a source is taken as it is. The admin page and `holon feed list` say `resumes from
  its position with cursor= in the URL`.
- **The query parameter goes last,** in place of any of its name the URL holds, its value
  percent-encoded: `?collections=app.bsky.feed.post&cursor=26984957930`. Nothing else in the
  connection changed: the position, the 1,000 ids and the lazy hold are 1.6a's and 1.6b's.
- **The sample** writes `mentions[address] = kept`, with `kept` absent unless the post mentions
  the word. Building it found that `if (kept !== undefined) mentions[address] = kept` was refused
  as doing nothing when it was the handler's only write, though the same `if` beside another write
  compiled and ran (trading-bot's): the compiler asked whether anything was left to tie to the
  handler's trigger, and the `if` had tied its write to `kept`. That was the compiler's check, not
  the feed's, fixed in its own change: a handler does something when it places a write or a call,
  however it is tied. A collection is read and written by key, `watch["word"]`. Its port is 8192:
  8191 is an e2e test's own flow.
- **The stand-ins** replay from `cursor=N`: what was sent from the message whose `seq` (or
  `payload.seq`) is N. The e2e's `ws-server.mjs` also writes what each connection asked for to
  `asked.log`.
- **The tests:**
  - The binding: each place, the cursor put in place of the URL's own, the header, the messages in
    place of the subscription (one read from a source), none for a new listener, the stand-in's
    replay; refused on SSE, without `--id`, with an id of two fields, with two places or a place
    and a message, a name that isn't one, and a written message without `{position}`.
  - The server test binds `posts` to the stand-in, deploys the sample, and sends four posts (a
    delete passed over by `--only`): three runs, two kept, at 4; the first connection asked with
    no cursor. Cut, with two posts sent while it is down: opened again with `cursor=4`, the 4 sent
    again passed over (on the timeline), the two run, each post once, three kept; the runs verify
    and the URL's key is in no log.
  - The e2e test deploys it in the editor against `ws-server.mjs`, sets the word, drops the posts,
    reads the server panel (`connected, at …, 1 passed over`), cuts the connections with two posts
    dropped meanwhile, reads the reconnect's `cursor=` in `asked.log`, the third mention, and the
    binding on the admin page.
- **Run live** on a scratch server (a release build, `--keep-runs 200000`), the feed bound to
  `wss://jetstream.us-east.bsky.network/…?collections=app.bsky.feed.post`, the word `the`: about
  25 posts a second ran as they came, the deletes passed over by `--only`. The server was killed
  at 08:09:12 and started again 40 s later: it asked from the position kept on disk (…957930), the
  timeline says `passed over an event of posts it ran already (26984957930)`, the backlog filled
  the buffer (1000 waiting) and was run through in 55 s. Of 6,172 runs, the widest gap between the
  times of posts run one after another is 0.3 s: the 4,515 posts written while it was down ran.
  16 ran twice: those after the kept position, run in the last 0.3 s before the kill, lazy and not
  yet synced, as `holon check` says; each wrote its item again. The runs verify. A first try on a
  debug build ran about 30 a second, only just as fast as Jetstream sends: after the restart its
  buffer stayed full, Jetstream ended the connection it read too slowly, and the host opened it
  again from the last post read.

#### 5.9.8 Slice 1.6d as built (2026-11-09)

As designed, with what building it found:

- **The binding** keeps `sequence` beside `key`. `/admin/feeds` and `holon feed add --sequence`
  take a field's path, a dotted one too, and refuse one that isn't a path. `--key` with neither
  `latest` nor `--sequence` is refused. The admin page and `holon feed list` say `numbered by
  trade_id, each product_id on its own`, and the admin page's form takes it (`feed sequence`, with
  the key's input shown for it as for `latest`).
- **The count is the reading thread's** (`Counting` in `subscriptions.rs`). It holds each key's last
  number read, the gaps `--only` passed over (carried to the next of their key that runs), and the
  keys read since the connection opened. It starts from the numbers kept when a link is made (a
  restart, a new version, a binding changed), and keeps its own across a reconnect, as the
  position does.
- **A number not above the last** begins the count anew only as the first of its key on a
  connection. A later one is a message sent again: its gap is 0 and the last is unchanged, so a
  replayed run of old numbers can't make the next message count again what already ran.
- **The buffer's drops still say their gap.** The design had a message `skip` dropped be no gap,
  which holds (it is `e.missed`), but the numbers the feed skipped before it would have gone
  unsaid. So the dropped event's gap is kept for its key (`Waiting.gaps`) and added to the next of
  its key that finds room. An event that replaces another (`latest`) adds the other's gap, as
  designed.
- **The numbers kept** (`Seen.numbers`, at most 10,000 keys, the least recent forgotten) move with
  the runs, as the position does: a strict run's at its end, a lazy one's once synced. One known
  limit follows, recorded and not fixed. After a restart, the messages of a key that `--only`
  passed over, or `skip` dropped, since its last run count in its next gap: they never ran, and
  the kept numbers don't know they were read. A flow that fetches its gaps fetches those too.
  Keeping the reader's numbers instead would cost a write for each message read. trade-tape isn't
  touched by it: `--only` passes over none of Coinbase's numbered trades.
- **Its state:** `Live.gaps` counts the numbers skipped since the link was made, across its
  reconnects: `connected, at BTC-USD/1117483559, 301 in gaps` on the server panel, `gaps` in
  `GET /deployments/<slug>/feeds`.
- **The sample** is as designed. The language asked for two changes:
  - `page` is absent when no gap asks for one, so is what is read from it, and `(page.status ===
    200 ? JSON.parse(page.body) : undefined) ?? []` falls to `[]`. A page that doesn't answer 200
    adds no trades, and the gap stays `unfetched`.
  - The count of what was fetched is `sizes.length`, of the `map`: a parsed page's own length isn't
    known to the compiler.

  Its port is 8193. Coinbase's REST API answers 400 (`User-Agent header is required.`) to a request
  without a User-Agent. A flow's request carries `ureq`'s own unless it sets one, so the sample
  sets none.
- **The stand-ins** (`sse.rs`, `ws.rs`) record what each connection asked for before they answer
  its handshake. Two of 1.6a's and 1.6c's tests read it as soon as the connection opened and failed
  now and then.
- **The tests:**
  - The binding: `--sequence` and `--key` kept and shown, a path refused, `--key` alone refused.
    The link numbers each message by its key, gives a filtered message's number, and gives none
    without the field.
  - The count: the next number, one skipped, several, per key, a key's first, a number not above
    the last (first on a connection, then again), and a gap `--only` passed over carried. Also
    `skip`'s dropped gaps carried to the next of their key, `latest`'s added, the 10,000th key
    forgetting the least recent, and a lazy position's numbers moving once synced.
  - The server test (`trade_tape_fetches_the_trades_its_feed_skipped_and_keeps_its_volume_exact`)
    binds `trades` to the WebSocket stand-in and the REST API to an HTTP stand-in whose trade sizes
    are their ids in thousandths, so a volume says exactly which trades were added. On one
    connection it skips 2. Cut, it skips one of each product. After a restart (the binding
    changed) it skips 1, then a gap past a page: 1,000 fetched, 91 unfetched. Each volume is exact,
    the runs verify, and the URL's key is in no log.
  - The e2e test deploys it in the editor against `ws-server.mjs` and a REST stand-in, then reads
    the server panel (`2 in gaps`, then `3 in gaps` after a cut with two trades lost) and the
    binding on the admin page.
- **Run live** on a scratch server (a release build) against Coinbase Exchange's matches for
  BTC-USD and ETH-USD:
  - It ran 36 s, then was killed. BTC-USD's kept number was 1117483263. It started again 38 s
    later.
  - The first trade of each product after the restart was its `last_match`, with a gap of 204
    (BTC-USD) and 97 (ETH-USD). Both pages were fetched and nothing was left unfetched.
  - Unbound about 50 s later: 676 runs. In each product, every run's gap is exactly the ids between it
    and the one before.
  - BTC-USD's volume, 4.90093013 over 616 trades (ids 1117483081 to 1117483696), equals the sum
    REST gives for the same 616 ids, and ETH-USD's 91.64382809 over 361 trades does too.
  - The 682 runs (with the `/volumes` calls) verify.

#### 5.9.9 Slice 1.6e as designed (2026-11-09, against `f2055a1`): a queue

§5.9.1 gives the queue its shape: SQS's API, a send that goes once the run's writes land, a
handler that deletes each message once its run is over. What it leaves open:

**The binding.** As a feed's, the URL with its key read from a source, never kept or shown:

```
HOLON_SECRET_PAYOUTS_URL=https://AKIA…:<secret, percent-encoded>@sqs.eu-west-1.amazonaws.com/123456789012/payouts
holon queue add payouts --url env:HOLON_SECRET_PAYOUTS_URL [--visibility T] [--keep-failed] [--for SLUG]
holon queue list
holon queue remove payouts
```

- **The URL is the queue's own** (`QueueUrl`), the access key id and secret its login,
  percent-encoded (a secret holds `/` and `+`). The region is the host's
  (`sqs.<region>.amazonaws.com`), or `?region=` for a service that copies the API under another
  name (ElasticMQ, LocalStack). `http://` only to this machine, as a feed's. A session token
  (`?token=`) is taken for temporary keys; an instance's own role waits for a job (fork 75).
- **Checked when bound:** `GetQueueAttributes`, signed, so a wrong key, a wrong region or a queue
  that isn't there is refused, saying what the service answered. It reads whether the queue is
  FIFO (`FifoQueue`, or a name ending `.fifo`) and its largest message (`MaximumMessageSize`),
  kept with the binding.
- **The binding keeps** the source, where it points without its login or query
  (`https://sqs.eu-west-1.amazonaws.com/123456789012/payouts`), `fifo`, `largest`, `visibility` (30 s to
  12 h, 5 minutes unless given) and `keep_failed`. `/admin/queues`, `holon queue`, and a
  **queues** list on the admin page's resources, as feeds have.

**The API.** SQS's JSON protocol (`application/x-amz-json-1.0`, `X-Amz-Target:
AmazonSQS.<Action>`), which AWS has answered since 2023 and ElasticMQ and LocalStack copy: a
JSON body each way, no XML (fork 76). Five actions: `GetQueueAttributes`, `SendMessage`,
`ReceiveMessage`, `DeleteMessage`, `ChangeMessageVisibility`. Each request is signed with
Signature Version 4 (`sigv4.rs`): the canonical request, the scope `<date>/<region>/sqs/aws4_request`,
the key derived by HMAC-SHA256 from the secret, as AWS's documents give it, tested on their
published example. Step 1.4's S3 signs with it too, its payload hashed the same way.

**Sending: `payouts.send(data, { group })`.** A statement, as `ops.send(…)` is:

- **The body** is `data` as JSON (text sent as it is), at most the queue's
  `MaximumMessageSize`, read when it is bound (256 KiB to 1 MiB); past it the run fails, before
  anything is sent.
- **Once the run's writes land,** as mail: spooled before the run's record is written (one file a
  message in `queue-spool/`, beside the bindings file), held until the record is on disk, then
  sent in order, a queue for each binding on its own thread. An unrecorded run's message is
  spooled and sent all the same.
- **Tried again** on what SQS says is temporary (no connection, a 5xx, a throttle), after 1 s,
  10 s, a minute, then every 5 minutes, for a day; refused (any other 4xx), or a day of trying,
  gives up. Each outcome is on the deployment's timeline, as mail's (fork 77). A restart sends
  what the spool holds whose run its log holds.
- **A FIFO queue** takes a message group (`group`, text): messages of one group come out in the
  order sent. Without one, the run's messages go in a group named for the deployment, so they
  keep their order. Its deduplication id is the run's id and the send's place in it, so a send
  tried again after a lost answer is one message. A standard queue has no such id: a retry after
  a lost answer can send twice, which its listener's once-per-message can't tell, since SQS gives
  each send its own id (fork 78).
- **At most 100 sends a run,** as mail's limit is; past it the run fails, before anything is sent.
- **A message computed from a secret** is refused, as a channel's pack is.

**Listening: `on.queue(payouts, (m) => …, { log })`.** One deployment listens to a queue, as to a
mailbox: a deploy is refused while another listens to it, or it isn't bound.

- **A thread for each deployment and queue** long-polls (`ReceiveMessage`, 20 s, up to 10, its
  visibility the binding's), runs the messages received one at a time in the order they came, and
  receives again once their runs are over. A FIFO queue's order holds.
- **Deleted once its run's record is on disk**, as a feed's position moves: a strict or
  unrecorded run's at its end, a lazy one's once the sync covering its record is done. A failed
  run's message is deleted too, its record saying why (fork 60), unless the binding says
  `--keep-failed`: then it comes back once its visibility runs out, and the queue's redrive policy
  counts it.
- **Kept out of sight while it waits or runs:** every half of the visibility, the reading thread
  extends what it received and hasn't deleted (`ChangeMessageVisibility`).
- **Once per message:** the last 1,000 message ids run on are kept beside the log
  (`queue-seen/<slug>/<queue>.json`), written as the record is; a message among them (one whose
  delete a crash cut off, or a standard queue's second delivery) is deleted without a run, on the
  timeline.
- **Unreachable:** tried again after a second, twice as long each time, up to a minute; the
  timeline says so once, and again when it is read again.
- **Its state,** on the server panel and in `GET /deployments/<slug>/queues`: `receiving`, how
  many wait for their run, and since the thread started how many ran, were passed over, and
  were kept for the redrive (`--keep-failed`): `receiving, 2 waiting, 120 run`.

`m` is the message:

| Field | |
|---|---|
| `data` | its body: a value when it is JSON, else its text |
| `id` | SQS's message id |
| `attributes` | its message attributes whose type is `String` or `Number`, as text |
| `sent` | when it was sent, in seconds since 1970, as `now` is |
| `count` | how many times the queue has handed it out (`ApproximateReceiveCount`) |
| `group` | a FIFO queue's message group, when it has one |

**The run** is `kind: "queue"`, its label the handler's (`on queue payouts`), the message sourced
as `QueueMessage("payouts")` with the clock and the random bytes. A parked run keeps it.

**The grant.** `QueueMessage("payouts")`, a driver: "run on each message from payouts
(https://sqs.eu-west-1.amazonaws.com/123456789012/payouts), and delete it once its run is over";
`QueueSend("payouts")`, a sink: "send messages to payouts". A flow may do both, to one queue or
two.

**The sample, `payout-events`** (fork 79): a payment provider's payout events, as a provider's
webhooks forwarded to SQS deliver them (`{"type": "payout.paid" | "payout.failed", "payout":
{"id", "amount", "currency", "account"}}`), each payout's status kept by its id; a failed one
sent to a FIFO queue `payout-retries`, its group the account, so one account's retries keep their
order. `GET /payouts` lists them. Strict, the default: a payout's status is money.

**Stand-ins.** The server test's SQS (`sqs.rs`'s, on loopback) keeps its queues in memory,
standard and FIFO, with visibility, receive counts and deduplication, and refuses a request whose
signature isn't the one its secret makes. The e2e's `sqs-server.mjs` does the same in Node.

**The live run** (fork 80): against AWS SQS with a key the owner makes (an IAM user whose policy
allows these five actions on two queues), or against ElasticMQ on this machine. The sample
deployed, its queue sent events from outside (the AWS console or `aws sqs send-message`), each
run once, deleted, the failed one's retry in `payout-retries`; the server stopped mid-batch and
started again, no message run twice.

**Tests:**

- **`sigv4.rs`:** AWS's published example, and a request the stand-in checks.
- **The binding:** each URL form, the region from the host or `?region=`, a token, refused
  without a login, plain HTTP elsewhere, a wrong key or a missing queue (the service's answer),
  `--visibility` out of range; `fifo` read.
- **Sending:** held for the record, in order, the group and deduplication id, tried again on a
  5xx and a throttle, given up on a 4xx, a restart sending the spool, the limit and the size.
- **Listening:** one run a message, deleted after its record (a lazy one's after its sync), a
  failed one deleted or kept (`--keep-failed`), a message received again passed over, its
  visibility extended through a long run, another deployment refused.
- **Server test**, the sample: events sent to the stand-in, each payout kept, the failed one's
  retry in `payout-retries` with its group, every message deleted; a restart with messages in
  flight runs none twice; the runs verify.
- **e2e test**, the sample in the editor against `sqs-server.mjs`.

#### 5.9.10 Slice 1.6e as built (2026-11-09)

As designed, with what building it found:

- **A queue's name is a flow's name:** letters, digits and `_`, as `import payout_events from
  "queue:payout_events"` takes it, so the binding names are `payout_events` and `payout_retries`;
  the SQS queues behind them keep their own (`payout-events`, `payout-retries.fifo`).
- **The send is one record,** `payout_retries.send({ data, group })`, as `ops.send({ … })` is,
  not `send(data, { group })`: the message is a value the run can build and pass. It lowers to
  `queue_send{queue}(message)`, whose payload is `{data, group}`. A message without `data`, or a
  `group` that isn't text of 1 to 128 characters (SQS's limit), has no value and sends nothing, as
  a mail that isn't one sends nothing. The body is `data`: text as it is, anything else as JSON;
  bytes, an empty body, a body past the binding's `largest` or a run's 101st send fail the run
  before anything is sent (`message not sent: …`).
- **A standard queue isn't sent the group:** SQS takes `MessageGroupId` only on a FIFO queue's
  send, so the group (the given one, or the deployment's name) goes to a FIFO queue alone, with
  the deduplication id `<run id>-<n>`.
- **Each queue's outbox keeps order strictly:** the message at its front is tried again while the
  ones behind it wait, so a group's messages reach a FIFO queue in the order sent even across a
  retry. A queue's outbox doesn't wait for another queue's.
- **The timeline says what is worth a person's look,** not each send (fork 77 had each outcome
  there, as mail's): a queue's first failure (`couldn't send a message to payout_retries yet: …;
  trying again`), a give-up, `sends to payout_retries again` once it takes one, and a message to a
  queue no longer bound. A deployment's events are kept for as long as it is, and a queue may take
  thousands of messages an hour. What was sent is counted instead, since the server started:
  `sent`, `gave_up` and `waiting` for each queue a deployment sends to.
- **Its state** is `GET /deployments/<slug>/queues`: `{listening: [{queue, label, address, status,
  since, error, waiting, run, passed_over, kept}], sending: [{queue, address, sent, gave_up,
  waiting}]}`. The server panel says each on a line (`receiving, 4 run, 1 passed over`; `2 sent`).
- **The listener** is as designed: a thread for each deployment and queue started by a reconcile
  every second, a second thread keeping what it received out of sight, the message deleted once
  its record is on disk (a lazy run's once synced, held with the feeds' positions). A message is
  added to the ids run on as it is deleted; a failed run's kept for the redrive is not, so it runs
  again when the queue hands it out again. Stopped mid-batch, the messages not run are made
  visible at once (`ChangeMessageVisibility` to 0). `m.sent` is whole seconds, as `now` is.
- **One deployment listens to a queue** by where its binding points, under whatever name: a deploy
  is refused while another deployment's handler listens to the same address.
- **The admin page** has a queues list and form in its resources (name, where the URL is kept,
  visibility, whether a failed run's message is left, for one deployment); permissions reads
  `QueueMessage` as "starts on each message it holds" and `QueueSend` as "sends messages to it".
- **The tests:**
  - `sigv4.rs`: AWS's published example, and `X-Amz-Date`'s form. `sqs.rs`: each URL form (the
    region from the host or `?region=`, a token, a port, plain HTTP refused off this machine, no
    login, no queue, a query it doesn't take), and the stand-in refusing a request signed with
    another secret.
  - `queues.rs`: a binding read from its source and shown without its key, refused bindings,
    the limits and the body, sends held until their record is on disk and then sent in order (a
    FIFO queue's group and deduplication id), retried and given up, a restart sending what the
    spool holds whose run the log holds, and a deploy refused while another listens.
    `dequeue.rs`: a message as the handler reads it, and the ids run on kept, the oldest forgotten.
  - The server test (`payout_events_runs_each_message_once_and_sends_a_failed_payouts_retry_to_its_fifo_queue`)
    deploys the sample against the stand-in: refused until both queues are bound; four events,
    four runs in order, each message deleted; the two failed payouts' retries in the FIFO queue,
    each in its account's group; one whose retry is past the queue's largest fails, writes nothing
    and is deleted. The first message handed out again is passed over and deleted, and the state
    says `run 4, passed_over 1` and `sent 2`. Stopped, a message put waits in the queue. Bound
    again with `keep_failed`, the waiting message runs and a failing one is left, comes back once
    visible, runs again and is left again. The runs verify (the failed ones not run again), and
    the key is in no log.
  - The e2e test deploys it in the editor against `sqs-server.mjs` (SQS's JSON protocol, each
    request's signature checked), drops two events, reads both payouts, both deletes, the retry
    in the FIFO queue in its account's group, the server panel's `receiving, 2 run` and `1 sent`,
    and the bindings and holders on the admin page.
- **Run live** (2026-11-09) against ElasticMQ 1.7.1, in Docker: SQS's JSON protocol, though it
  doesn't check a signature. Both queues bound (it answered `GetQueueAttributes`, the retries'
  FIFO, 256 KiB), the sample's deploy refused until they were. Beside the sample, a flow sending
  events with `payout_events.send` and one listening to the retries. 540 events, 40 through that
  flow and 500 put on the queue: 540 runs of 540 message ids, each message run once, and the
  queue empty after.
  - The server killed mid-batch: the two messages it had received and not run came back once
    their 30 s were up, and ran then, after later ones. A standard queue's order, which the
    retries followed.
  - The binding removed mid-batch, five times: what the listener held and hadn't run was visible
    again at once (`ChangeMessageVisibility` to 0), 15 messages handed out again, each run once.
  - 85 retries of the first 320 events in the FIFO queue, each in its account's group, a group's
    in the order its runs sent them. The kill held one group back for the binding's 5 minutes:
    a FIFO queue keeps a group waiting while one of its messages is out of sight.
  - Both deployments' runs verify (`holon verify`, 342 and 89 runs), and the key is in no file
    the server wrote.
- **Run live against moto** 5.2.5 (2026-11-09), in Docker, checking keys as IAM does
  (`INITIAL_NO_AUTH_ACTION_COUNT`): each request's signature computed again by botocore's signer,
  the key one its IAM made, a temporary key's session token matched, the key's policy evaluated
  for the action. (moto evaluates SQS's actions against the resource `*`, so the policy there names
  the actions and not the queues.) A user allowed the five actions, one allowed all but
  `SendMessage`, and a role the first may assume.
  - A wrong secret is refused at bind with what moto answered, `SignatureDoesNotMatch`. That
    took a change: moto refuses a key in the query protocol's XML (`<Code>`, `<Message>`), which
    the client didn't read, so it said only `answered 403`; it reads both now (`sqs::said`).
  - Bound with the first user's key (moto's queues take 1 MiB, which the binding read), the
    sample, the flow sending events and the one reading the retries ran 20 events: 20 runs, 5
    retries in their accounts' groups, both queues empty after.
  - The retries bound for payout-events to the key without `SendMessage`: a failed payout's run
    succeeds, and its retry is given up on the timeline, `answered 403 to SendMessage:
    AccessDenied: …`; the state says `gave_up: 1`.
  - The events queue bound again to the role's temporary key (`?token=`): 20 more events
    received, run and deleted, and sent by the flow sending them, with it.
  - The three deployments' runs verify (43, 41 and 16), and none of the six secrets and tokens
    is in a file the server wrote.
  - Real AWS is still to come: no account yet.

#### 5.9.11 Slice 1.6f as designed (2026-11-09, against `ea55171`): a Postgres `LISTEN` channel

A feed whose messages are a Postgres channel's notifications: what a trigger, or any session,
says with `pg_notify('ledger_entries', …)` as its transaction commits. The flow is unchanged from
a feed's: `import entries from "feed:entries"`, `on.feed(entries, (e) => …)`.

```
holon feed add entries --database ledger --channel ledger_entries
                       [--id FIELD,…] [--only FIELD=VALUE]… [--sequence FIELD [--key FIELD]]
                       [--buffer N] [--on-full …] [--idle T] [--for SLUG]
```

- **On a bound database,** by its name, in place of a URL: the binding the listening deployment
  sees as `ledger` (its own, else the server's), read each time the connection opens, so a
  database bound again moves the feed with it, and one no longer bound leaves the connection
  reconnecting, saying why. Postgres alone: a database of another engine is refused when the feed
  is bound (`LISTEN is Postgres's: the database ledger is sqlite`).
- **`--channel`** is the channel as `pg_notify` names it: letters, digits and `_`, up to 63 bytes
  (Postgres's names), quoted when listened to, so its case counts as `pg_notify`'s does.
- **What a URL's binding says that a channel's doesn't** is refused with it: `--url`,
  `--header`, `--subscribe`, `--heartbeat` and `--resume`. `--id`, `--only`, `--sequence`,
  `--key`, `--buffer`, `--on-full` and `--idle` read a notification's JSON as they read a
  WebSocket message's.
- **Checked when bound:** connected with the database's connection, `LISTEN` on the channel,
  closed. Shown as the database is, with the channel: `postgres://db.example.com:5432/ledger,
  channel ledger_entries`; the connection is never shown.
- **Its connection is its own,** one a deployment and feed, not one of the database's pool: a
  `LISTEN` belongs to its session. TLS as the pool's (`engines::pg_tls`). The reading thread waits
  for a notification a second at a time, to notice a close; after `--idle` (90 s) with none it asks
  the server whether the connection is still there (`is_valid`), and opens it again only when the
  server doesn't answer: opening it again for quiet would lose what was sent meanwhile.
- **`e`** is the feed's event: `data` the payload (a value when it is JSON, else its text),
  `event` the channel, `id` by `--id`, `gap` by `--sequence`.
- **No position.** Postgres sends a notification to the sessions listening when its transaction
  commits, and keeps none: what commits while the connection isn't there (a restart, a lost
  connection, before the deployment) is never sent. The timeline says so when it listens again
  (`listens to the feed entries again: what was sent meanwhile is lost`). `--sequence` counts what
  was lost by a key (`e.gap`, across a restart too); a flow that keeps its own last number (as
  `ledger-watch` does) needs none, and reads what it missed from the database it is bound to.
- **What Postgres does,** said in the guide: notifications come in the order their transactions
  committed; one rolled back sends none; identical ones (channel and payload) of one transaction
  are sent once, so a payload that names its row keeps each; a payload is 8,000 bytes at most, so
  a big row is named by its id and read.
- **Several deployments** may listen to one channel, each on its own connection: a notification
  is a broadcast.
- **The grant** is the feed's, `FeedEvent("entries")`, shown "run on each event from entries
  (postgres://db.example.com:5432/ledger, channel ledger_entries)". Listening needs no grant on
  the database; reading it, as the sample does, is the database's own.
- **Engine:** `listen.rs`, the `postgres` crate's notifications (`timeout_iter`) on a client of its
  own.
- **The editor:** the admin page's feed form takes a database and a channel in place of a URL.
- **The sample, `ledger-watch`:** a ledger's entries, each account's numbered one after another
  (`seq`), said by a trigger as they commit; the handler adds each to its account's balance,
  reading first the entries of the account it hasn't added (all of them for an account new to it,
  or those committed while it wasn't listening). The balance stays exact across a restart and a
  lost connection, without the feed knowing where it was.
- **Tests:** a server test against the Postgres `HOLON_TEST_POSTGRES` names, skipped without one
  as the database tests are: refused for a database not bound, of another engine, and a channel
  that isn't one; the sample deployed, entries inserted and run in the order committed; the
  listener's session ended by the server (`pg_terminate_backend`), the entries committed meanwhile
  read by the next run, the timeline saying the connection was lost and listened to again;
  balances exact; the runs verify. The e2e test deploys the sample in the editor against a
  Postgres beside the job's MongoDB (`start-postgres.sh`), the entries inserted by a second
  deployment that writes the ledger.

#### 5.9.12 Slice 1.6f as built (2026-11-09)

As designed, with what building it found:

- **The binding keeps the database's name, not its connection:** `Binding` has `database` and
  `channel`, and its `url` is empty (not kept). `Binding::seen_by(databases, slug)` fills `url`
  with the connection source of the database `slug` sees by that name, and `address` with its
  address and the channel, when it is a Postgres; else the URL stays empty and a connection says
  `the database ledger isn't a Postgres bound for this deployment`. The reconcile maps each binding
  through it, so a database bound again, or removed, is a changed binding: the connection is
  opened anew, as for a feed bound again.
- **`Feeds::bind` takes the database** as the deployment (or the server) sees it, resolved by the
  admin route (`Databases::resolve`), since feeds.rs holds no databases. A channel's binding
  without its database, with both or neither of `--database` and `--channel`, or with a URL's
  options is refused before anything is reached.
- **Closing it:** a Postgres client has no socket the host can shut from another thread, so
  `sse::Closer` became `Socket(TcpStream)` or `Flag(Arc<AtomicBool>)`: a `LISTEN` connection reads
  a second at a time (`timeout_iter`) and looks at its flag between, so a stopped deployment's
  session ends within a second.
- **What it says** is what Postgres said (`engines::pg_said`): a server's error by its message,
  not the client's `db error`, and the client's own with the cause its text leaves out, which the
  pool's errors now say too: `lost the feed entries: the database can't be reached: error
  connecting to server: No connection could be made because the target machine actively refused
  it. (os error 10061); trying again in 1s`.
- **Each connection opened again says what it lost,** whether it was lost or couldn't be opened
  for a while: `listens to the feed entries again: what was sent meanwhile is lost`. A URL's feed
  says `reads the feed … again` only after a failure, since it resumes.
- **The grant's words are the feed's** (`run on each event from the feed "entries"`), as every
  feed's: the address isn't in them, it is the binding's, shown by the admin page, `holon feed
  list` and the server panel.
- **`e.event`** is the channel, and `e.data` the payload, parsed when it is JSON.
- **The editor:** the admin page's feed form asks "from a URL" or "a Postgres channel", the latter
  with the Postgres databases bound as its choices, a channel, and `--id` and `--only`; a bound
  channel reads `listens on postgres://…/ledger, channel ledger_entries (the database ledger)`.
- **The e2e test** has its own ledger writer, `ledger-writer`, deployed with the command line, the
  ledger bound read-write for it alone (`--for`); the sample's binding is read-only, its schema
  read from the sample's comment (as the server test reads it), so the comment can't drift. What
  isn't heard is made by stopping the deployment (`/stop`, then `/run`) between two entries. CI's
  e2e job starts `postgres:17.7` on 5433 (`start-postgres.sh`) and removes it, with its volume,
  after the run.
- **Run live** against Postgres 17.7 in Docker (WSL), a database `ledger` bound read-only with the
  sample's `ledger.sql`, a server of the slice's build on loopback: three accounts written with
  `psql` as another service would, in single rows, a statement of three and a transaction of two;
  a transaction rolled back sent nothing; the listener's session ended with
  `pg_terminate_backend`, an entry written in the second before it listened again, read by the
  next run (`read: 1`); the server killed, three entries written for two accounts while it was
  down, one of them new, each read by its account's next run once it was back; Postgres restarted
  (`docker restart`), and stopped for a few seconds then started, the listener back on its own each
  time and the next entry heard. Each balance equal to the ledger's own `sum(amount)` by account;
  the 18 runs verify offline; no file the server wrote holds the database's password.

### 5.10 A bytes value, and decimals with it

Not a capability: a value kind, which several capabilities stand on. Binary files can't be read
(§1), and half of this list produces binary data: PGP (§5.3), zip and gzip, mail attachments
(§5.2), objects in a bucket (§6), a socket's messages (§5.9), signatures over raw bytes. Hex or
base64 text would carry them, but doubles their size (a 1 MiB file limit holds 512 KiB) and makes
every leaf agree on one text form forever. So a **`Bytes` value**.

**It is cheaper than a new kind sounds:**

- **No unit is hashed differently.** The canonical encoding is tagged (`value_hash.rs`: `Number`
  1, `Boolean` 2, `Text` 3, `List` 4, `Effect` 5, `Sum` 6, `Record` 7), so `Bytes` takes tag 8 and
  no existing value's encoding moves. Nothing deployed needs redeploying.
- **The compiler finds every place it must be handled:** `Value` is matched exhaustively.
- **Modules get it for free:** a WebAssembly module's ABI is that same encoding
  (`holon-module`).
- **No new syntax:** bytes come from leaves (`from_hex`, `from_base64`, `utf8_encode`) and from
  capabilities (a file read as bytes, a socket's message, an attachment).

**What it must settle is every boundary where a value becomes JSON or JavaScript:**

- **The log and export:** bytes as base64 in the JSON lines, large ones kept in `blobs.rs` as file
  texts are.
- **HTTP bodies and `json_stringify`:** JSON has no bytes. `json_stringify` refuses them, so a
  flow says `to_base64(b)` where it means it, and `json_parse` never makes them; an HTTP body that
  is bytes is sent as they are.
- **The wasm bridge and the editor:** a `Uint8Array` in JavaScript, shown as its length and a hex
  preview.
- **Leaves:** `length`, `slice`, `concat` and equality on bytes; to and from hex, base64 and
  UTF-8; `sha256` and `hmac_sha256` over bytes as over text; later, readers for binary fields
  (`u16be` and the like) for ISO 8583.
- **Types:** `Type::Bytes`, and how a record or a JSON shape holds one.

**Decimals are decided in the same L4** (§8, fork 1): a `Decimal` value raises the same questions
(its JSON form, the log, the bridge, the editor, which leaves take it), so the answers are worked
out once, for both.

#### 5.10.1 As designed (2026-11-08, against `331ea13`)

**The value.** `Value::Bytes(Vec<u8>)`, its type `Type::Bytes`. Its value tag is 8 and its type
tag 10 (`value_hash.rs`, `hash.rs`), after every tag in use, so no hash moves for them. Two
are equal when their bytes are (`===`, `equals`, a record key's value, a memo key), as Holon's
lists and records are, where JavaScript compares a `Uint8Array` by identity. `typeof` is
`"object"`, it is true as a condition (an empty one too, as JavaScript's `Boolean(new
Uint8Array())`), `String(b)` is its bytes as numbers joined with `,` as JavaScript writes it, and
`to_text` writes `bytes(68656c6c6f)`, its bytes in hex (the editor's `formatValue` the same).

**Its leaves** (7 new, and 4 that take bytes as well as what they took). Each gives nothing for a value it
can't take, as the others do:

| Leaf | Gives | Its JavaScript form, where it has one |
|---|---|---|
| `text_to_bytes(text) {encoding: utf8, hex, base64, base64url}` | the bytes; nothing for hex or base64 that isn't | `new TextEncoder().encode(s)`, `Uint8Array.fromHex(s)`, `Uint8Array.fromBase64(s, { alphabet })` |
| `bytes_to_text(bytes) {encoding}` | the text; nothing for bytes that aren't UTF-8, as a `fatal` decoder throws | `new TextDecoder("utf-8", { fatal: true }).decode(b)`, `b.toHex()`, `b.toBase64({ alphabet, omitPadding })` |
| `bytes_length(bytes)` | how many | `b.length`, where `b` is known to be bytes (as `.length` picks a text's or a list's) |
| `bytes_concat(a, b, …)` | them joined | none (Node's `Buffer.concat`); by name |
| `bytes_to_list(bytes)` | the bytes as numbers 0 to 255 | `Array.from(b)`, `[...b]` |
| `list_to_bytes(list)` | the bytes; nothing if an item isn't a whole number from 0 to 255, where JavaScript wraps it | `new Uint8Array(xs)`, `Uint8Array.from(xs)` |
| `file_read_bytes(name) {folder}` | a file's bytes, any file | `folder.bytes(name)`, as a `Blob`'s `bytes()` |
| `sha256`, `hmac_sha256`, `secure_equals` | over bytes as over a text's UTF-8 | their ports are `Any`: a text or bytes |
| `slice` | bytes from `start` to `end`, negative from the end, as a list's | `b.slice(start, end)` |

`base64` pads and `base64url` doesn't, as JWTs and the `base64` leaf (whose `url` alphabet is
unpadded) do; either decodes with or without padding. The JavaScript forms are the reader's to add
one by one, each a line in `ts.rs` with its test (the theme's §10); this step reads `.length` and
`.slice` on known bytes, and the rest are called by name.

**A file read as bytes.** `file_read_bytes` asks the host as `file_read` does, with `bytes: true`:
the host reads the file whole, under the same limits (`inline_file_bytes`, `max_file_read_bytes`),
and answers its bytes whatever they are. The log keeps it as it keeps a text: inline when small
(the log's JSON writes bytes as base64, below), else in the blob store by the BLAKE3 of the bytes,
which `holon verify` reads back as bytes. Writing bytes to a file waits for the first flow that
writes a binary file (1.2's zip, 1.4's buckets).

**Every boundary where a value becomes JSON or JavaScript:**

- **`json_stringify` refuses bytes** (nothing): JSON has none, and a flow says `bytes_to_text(b, {
  encoding: "base64" })` where it means that. `json_parse` never makes them.
- **An HTTP body that is bytes is sent as they are**, `application/octet-stream` unless the flow's
  headers say otherwise. A request's body as bytes (uploads) is its own step: the request record's
  `body` stays text.
- **The log, an export, the control plane's JSON (`SValue`):** `{"type": "Bytes", "value":
  "<base64>"}`, so a log line stays JSON and verify reads the same bytes back.
- **The wasm bridge (`WValue`):** a `Uint8Array`; the editor shows `formatValue`'s form.
- **State and collections:** storable as any value is, kept in the value encoding.
- **Databases:** bytes as a parameter are a binary one (Postgres `bytea`, MySQL's bytes, Mongo's
  binary); a `bytea` column reads as bytes, where it read as hex text. A module's ABI is the value
  encoding, so a WebAssembly module gets bytes with tag 8.
- **Secrets:** a value that holds a secret's UTF-8 in its bytes is one that holds the secret, for
  redaction and for verify, as a text that holds it is.
- **Types:** `Uint8Array` is the type of bytes in `holon.d.ts` and in a function's annotations;
  `WType::Bytes` on the canvas, a port's colour of its own.

**Decimals: leaves over decimal text, in step 1.2, not a value kind** (§8, fork 1, decided). The
places Holon meets exact decimals already hand it text: a Postgres `NUMERIC` column reads as its
exact text (`engines.rs`), and CSV, XML, and the amounts exchanges and many payment APIs send (as
JSON strings, for this very reason) are text. A `Decimal` value would add a JSON form, a place in
the log, the bridge and the editor, and a decision for every leaf that takes a number, and the
language's syntax has no decimal for it: JavaScript has none, so `+` on one would need a branded
`number` type in `holon.d.ts` and a meaning for `Decimal + Number`, and `===` would have to choose
between `1.10` and `1.1`. Leaves over text (`decimal_add`, `decimal_compare`, `decimal_round {scale,
rounding}`, `decimal_sum`, …) need none of that and are exact. What they can't do is write an exact
JSON number token: an amount goes out as a string, or through `Number(…)` as a JavaScript number,
exact to 15 digits. The cost to keep in mind: `a + b` on two decimal texts joins them, as
JavaScript's does; 1.2's design says how a flow is kept from it. A `Decimal` value stays possible
later, on a new tag, if a job needs exact number tokens.

**The sample, `file-checksums`:** files dropped in a folder, each one's SHA-256 over its bytes
(any file: an image, a PDF) kept in a manifest, read back by a route.

#### 5.10.2 As built (2026-11-08)

As designed, with what building it found:

- **Three leaves' hashes moved.** The tags move nothing, but `sha256`, `hmac_sha256` and
  `secure_equals` now take `Any` where they took `Text`, so their units, and every flow using them
  (guarded-api, guestbook, idempotent, live-board, marketplace, sign-payload, slugs, witness),
  have new hashes. Dev phase: nothing is kept for the old ones.
- **`String(b)`** is `as_text`, which writes bytes as JavaScript does (`1,22`); `+` with a text
  on one side gives nothing for bytes on the other, as it does for a list or a record.
- **The log:** a file read as bytes that is kept in the blob store is also left out of the
  timeline, as a kept text is (a node carrying it would hold its base64 whole). A line holding
  bytes is always redacted by walking it: its base64 would hide a look-alike of a mark from the
  shortcut that skips lines without a `‹`. Redaction and the export's check for secrets walk each
  run of valid UTF-8 inside bytes, where a secret's UTF-8 can only be.
- **The export** keeps a file read as bytes apart from the texts: `blob_bytes`, padded base64 by
  blob, read back as bytes and checked against the file's hash; with `--files`, verify reads the
  file given as bytes. A tampered byte fails the run that read it.
- **Databases:** SQLite's blob, a Postgres `bytea`, MySQL's binary columns and Mongo's generic
  binary read as bytes (another Mongo subtype, a UUID's, stays its hex); bytes as a parameter are
  SQLite's blob, Postgres's `\x` hex where the statement takes a `bytea` (refused elsewhere),
  MySQL's bytes and Mongo's generic binary.
- **The wasm bridge** reads bytes from a `Uint8Array`, an array of numbers, or padded base64, as
  the server's log writes them, so the editor's replay of a logged run takes them as they are.
- **Not yet:** an `http_request` body that is bytes gives nothing, as before (it takes a text or
  JSON); uploads are 1.4's. Writing bytes to a file waits for 1.2 and 1.4, as designed.

Run live on a scratch server: a PNG, a zip, a PDF-headed file with bytes that aren't UTF-8, 100 KB
of random bytes and a text, dropped in the folder, each answered its size, its kind and the
SHA-256 `sha256sum` prints; the export verified (the two big files' bytes from `blob_bytes`), a
changed byte failed its run, and an export without the files verified with `--files`.

### 5.11 Static files: a route for the rest of a path, and an answer that is a file's bytes

Designed 2026-11-04 against `450e3de` and re-checked the same day against `ff03a80` (`main` after
the request path's work), cited by file and line there. Step 1.8. Builds on
[named folders](named-folders-L4.md) (handles, reads by hash), [channels](channels-L4.md) (an
answer the host streams after the run) and [the request path](request-path-L4.md) (C5, a log level
per handler, §5.11.5).

Two features that together let one flow serve a built web app (a Vite or React `dist/`) and its
API:

- **A rest segment** (`*path`): a route that matches the rest of a path, however many segments.
  Routing, not a capability.
- **Answering with a file** (`site.serve(name)`): a route answers with a file's bytes from a
  named folder, any size, any type (text or binary), streamed by the host and never through the
  flow. A new item, `FileServe(folder)`, so §5.11.6 answers §4's checklist.

```
flow site
port 8131
folder site

route GET /api/hello (req) {
  respond { hello: "world" }
}

route GET / (req) lazy {
  respond site.serve("index.html"), missing: 404
}

route GET /*path (req) lazy {
  respond site.serve(req.params.path) ?? site.serve("index.html"), missing: 404
}
```

(`lazy`: each request is recorded, on disk within about a second, §5.11.5.) `check` says:

```
a server on :8131
  GET /api/hello
  GET /
  GET /*path
it may:
  receive requests
  read files in the folder "site"
  answer its callers with files from the folder "site"
```

The bytes a caller gets are the bytes of the file whose hash the run recorded, or the caller gets
nothing.

#### 5.11.1 What is already there

- **Routes** are parsed by `parse_path` (`holon-core/src/declare.rs` 385): literals and `:name`
  params, nothing else. `match_route` (412) matches only routes with the same number of segments
  as the request, and the most literal segments wins, ties to the earlier route. Two routes
  clash when their segments are the same with every param treated alike (320).
- **Three other copies of the path's shape** have to agree with core: the language reads a
  route's path as a run of tokens (`holon-author/src/lang/parse.rs` 584); the wasm bridge hands the
  editor the segments (`holon-wasm/src/lib.rs` 900, `route_path`); and the editor matches requests
  itself for its request tester (`editor/src/endpoint.ts` 20, 43–52, 119). The author's routes
  module has its own shape for clashes (`holon-author/src/routes.rs` 236).
- **A response** is read from the endpoint's output 0 (`holon/src/data.rs` 163): a value, an
  envelope `{status, headers, body}`, an effect (204), or a **subscription**: a
  `channel_subscribe` effect, recognized by `subscription` (51) and streamed by the transport. The
  flow can't set framing headers (`FRAMING`, 48). A subscription is the precedent for an answer
  the host delivers after the run, from a value the run computed.
- **Letting callers subscribe is its own permission,** `ChannelRead("updates")` (`kind.rs` 151,
  337), described as "let callers subscribe to its channel" (`holon-author/src/describe.rs` 48),
  apart from the flow sending on it. Answering with a secret is likewise its own,
  `SecretAnswer(name)` (`kind.rs` 169). Publishing something to callers is shown and approved on
  its own.
- **A handle** (`inbox.file(name)`, `Folders::handle`, `holon/src/folders.rs` 503) is
  `{folder, name, size, hash, modified}`: the file hashed whole, its bytes kept nowhere. The run
  log keeps the handle as a read.
- **A file sent by handle** is `http_upload` (`primitives.rs` 986): the host opens the file,
  checks it against the handle's hash before a byte is sent, and again as it sends
  (`folders.rs` 1559–1578, `Sent`; `outbound.rs` 46, `FileBody`). A file that changed fails the
  call with `changed`. The log keeps the handle, never the bytes.
- **The data plane** is hyper (http1) on tokio (`holon/src/dataplane.rs`), whose `Answer` (174) is
  a whole body, a forwarded stream or an event stream. A file is a fourth kind.
- **The log** keeps a response's body as text cut at 64 KiB (`recorder.rs` 97), and every
  request, routed or not (`listeners.rs` 255).
- **Each handler has a log level** (request-path C5, built): `strict` by default (the record on
  disk before the answer, about 0.6 ms of fsync, C2 e), `lazy` (on disk within about a second),
  or `unrecorded` (counted, never logged, failed or not, resilience-audit §2.8; approved by a
  person as `Unrecorded("GET /health")`). Only a handler that may record lightly takes a word:
  every capability sourced, none `Await` (`may_record_lightly`, `declare.rs` 86). A request no
  route took is `lazy` (request-path Q12).
- **Some routes run inline,** on the async worker instead of the blocking pool: only those that
  read nothing but `NetworkIngress`, the clock and randomness (`READS_HERE`, `dataplane.rs` 381).
  A file read isn't among them (resilience-audit R2 took collection reads out for the same
  reason: a disk can be slow).

#### 5.11.2 The rest segment

**Syntax.** `*name` as the last segment of a route's path: `/app/*path`, `/*path`. One per path,
only last; anything else is a bad path, as a repeated param name is today.

**What it matches.** One or more segments, the rest of the path. Never zero: `/app/*path` doesn't
match `/app` or `/app/`, as `:id` never matches an empty segment (`declare.rs` 432). A flow that
wants `/` too declares `GET /` beside `GET /*path`, as the example does.

**Its value.** `req.params.path` is the rest, its segments percent-decoded one by one and joined
with `/`: `/app/css/site%20a.css` gives `css/site a.css`. A segment that decodes to contain `/`,
or that is `.` or `..`, makes the route not match (404). Folders refuse such names anyway
(`folders.rs`, `bad_file_name`), but the parameter should never carry a path that means something
other than what the request said.

**Which route wins.** A route without a rest segment beats any route with one, whatever their
literals. Among rest routes, the most literal segments wins (the longest prefix). Ties go to the
earlier route, as now. So `GET /api/hello` and `GET /api/:name` both beat `GET /*path`, and
`GET /docs/*page` beats `GET /*path` for `/docs/a/b`.

**Clashes.** Two rest routes clash when their segments before the rest are the same with params
treated alike. A rest route never clashes with a route without one.

**Status for other methods.** A 405's `allow` header lists the methods with a matching route,
rest routes included (`data.rs` 104–114, unchanged).

**What it changes about what a flow may do.** Nothing: the route is listed like any other
(`GET /*path`), the request is logged as now, and its param is text from the request. One thing
does change in practice: a flow with `GET /*path` runs a handler for every GET, so requests that
would have been 404s without a run now run. That is the flow's choice, shown in its route list,
and a `lazy` rest route keeps them as cheap as 404s were (§5.11.5).

**Where it is built.** `Segment::Rest(name)` in core (`parse_path`, `match_route`, the clash
check); the token in the language's route path (`Tok::Star`, `parse.rs` 584) and the printer; the
wasm bridge's `route_path` (`rest: Some(name)`); the editor's `parsePath` and matcher, kept equal
to core's by a test over the same table of paths and requests; `routes.rs`'s shape. Docs:
`LANGUAGE.md` (routes), the manual's reference.

#### 5.11.3 Answering with a file

**The language and the node.** `site.serve(name)` lowers to
`file_serve{folder: site}(file_handle{folder: site}(name))`:

- **`file_handle`** is today's leaf. It hashes the file and is logged as a read; a missing file is
  absent, so `missing: 404` and `??` work as for any read.
- **`file_serve`** is a new primitive. Pure, like `channel_subscribe` (`catalog.rs` 353), it
  turns a handle into an effect value, `Effect { sink: "file_serve:site", payload: {folder,
  name, size, hash, type} }`. `type` is the content type core reads from the name's extension
  (a fixed table in core, so it is part of the run and replays). It's absent when the handle is
  absent or names another folder.
- **Its capability** is `FileServe("site")`, a sink like `ChannelRead`, described as "answer its
  callers with files from the folder \"site\"". `FileRead("site")` still comes from the handle
  leaf. A deploy asks for both; a host policy can refuse either.

`respond site.serve(n)` answers with the file. So does an envelope whose body is one, and the
envelope's headers are added:
`respond site.serve(n), headers: { "cache-control": "max-age=31536000, immutable" }`.

The wiring form can feed `file_serve` any handle, for example a handle a flow computed from a
listing. The host checks that the handle's folder is the node's folder and one the deployment
reads, as `Sender` checks an upload's (`folders.rs` 1505).

**What the host does.** `respond` (`data.rs` 163) recognizes the effect as it recognizes a
subscription, and returns a `DataResponse` with a new field, `file: Some(Served {folder, name,
size, hash})`. After the run has committed, the transport (`dataplane.rs`) answers:

1. **Headers.** `content-type` from `type` unless the flow set one; `content-length` the
   handle's size; `etag: "<hash>"`; `cache-control: no-cache` unless the flow set one
   (revalidate each time, which with the ETag costs a 304).
2. **Conditional requests.** `If-None-Match` that names the hash: 304, no body. The run still
   ran: which file a path serves is the flow's to say, every time.
3. **The bytes.** Opened through the deployment's `Scoped` folders (confined as every read is),
   checked against the handle:
   - **Small files** (up to `inline_file_bytes`, 1 MiB by default) are read once, hashed, and
     sent from memory if the hash matches. The bytes hashed are the bytes sent.
   - **Bigger files** stream, hashed as they go, after a check of size and modified time against
     the handle. If the hash doesn't match at the end, or the size isn't the handle's, the
     connection is cut before the last chunk. The caller's client sees a short body against the
     `content-length` and drops it.
   - A file that changed before a byte went out answers **503** with `retry-after: 1`. The log
     says `changed`, as an upload's does.
4. **HEAD and ranges** are a later slice (§5.11.8).

**The handle's cost.** A handle hashes the whole file on every request: about a millisecond per
few MB with BLAKE3. For a bundle of a few hundred KB that doesn't matter; for video it does. The
host may keep a handle cache by `(folder, name, size, modified, inode)`. It is safe because the
send always checks the bytes against the hash: a cache that lies (a file rewritten within the same
second, at the same size) gets a 503 or a cut connection, never wrong bytes under the right hash.
It is an optimization, and it isn't in the first slice.

**Files, and bytes a run computed.** This is not a binary value in core; `Bytes` is (§5.10, step
1.1, fork 21), and the two fit together:

- **`serve` is for files.** A file of any size never enters a run whole (named-folders §4). The
  run holds a handle, and the host sends the bytes. That stays true after `Bytes` lands: a 40 MB
  video is never a value.
- **`Bytes` is for what a run computes**: a generated PNG, a zip, a signed PDF, within the run's
  limits. Once value kinds land, `respond b` with a `Bytes` value sends the bytes as they are
  (§5.10: "an HTTP body that is bytes is sent as they are"), with `application/octet-stream`
  unless the flow sets a type. Neither needs the other.

**Folder backends.** A folder bound to SFTP or S3 (§6) serves the same way: the host streams from
the backend and checks the hash as it sends. Redirecting the caller to a presigned S3 URL would be
faster, but the host would never see the bytes, so it couldn't stand behind the hash. Refused
(§5.11.9).

#### 5.11.4 The log, replay and verify

- **The run** is logged at its route's level (§5.11.5), as any request's: the request, the handle
  read (`{name, size, hash, modified}`, tens of bytes), and output 0, the `file_serve` effect.
- **`response_body`** is `file site/assets/app.3f9a1c.js (148.2 KB, blake3 7c1e…)` instead of the
  bytes; `status` is what was sent (200, 304, 503).
- **Replay and verify** re-run from the logged handle and compare output 0. The bytes the caller
  got are proven by the hash in the chain. Whether they can be produced again depends on the
  folder: a folder keeps reads `bytes` or `hash`, and a handle is always kept by hash
  (named-folders §6). A file served a thousand times costs a thousand small lines, never a
  thousand copies.
- **A cut connection** (§5.11.3, step 3) is logged with `error: "changed while sent"`.

#### 5.11.5 Limits, and log levels

- **Size:** a deployment's `max_served_bytes` per file (default 1 GiB, capped by the host policy),
  past which `file_serve` answers 500 and the log says why. Nothing about the size enters a run.
- **Connections:** a streaming answer holds a connection, so it counts toward `max_connections`
  (`dataplane.rs` 48) as a subscription does.
- **A run per request.** Every file request runs the flow. How much it costs is the route's log
  level (below). A deployment serving many assets should still sit behind a CDN, and then Holon
  records what reaches it, not every visitor.
- **The pool's hop.** A file route reads the disk (the handle hashes the file), so it runs on the
  blocking pool, never inline (`READS_HERE`, §5.11.1), whatever its level.

**`FileServe` may record lightly.** As C5 is built, a file route could only be `strict`:
`FileServe`, like `ChannelRead`, is performed, not sourced, and `may_record_lightly` takes nothing
performed. Every asset of a page would then wait for an fsync, about 0.6 ms each, and a 304 as
much as a 200.

C5's rule stands for a reason (request-path I-g): **nothing that changes anything goes unrecorded
or lazy**. Its table lists what breaks without a record: state and its checks, waits, channel
packs rebuilt from the log, a callee's record, something done to the world. Answering a caller
with a file breaks none of them. It changes nothing in Holon or outside it; it is an answer, as a
text body is, and a text body is allowed `lazy`.

So `may_record_lightly` also takes `FileServe`: a new `Capability::is_answer()`, true for
`FileServe` only, checked beside `is_sourced`. `SecretAnswer` stays out: C5 excludes it on
purpose. This amends a rule the request path built (fork 22).

- **`lazy`:** each request is recorded (the handle, the hash, the status) and on disk within about
  a second. Even a recorded file route keeps a short line, never the 64 KiB of a page body that
  C5's "large answers" worried about (§5.11.4).
- **`unrecorded`:** a person approves "keep runs of GET /*path out of its log, failed or not (they
  are counted)". What was sent is then not recorded, by that person's decision (request-path
  I-h). A file that changed while sent counts as a failure in the tally, its last error kept.
- **A rest route takes what would have been 404s.** Those were `lazy` (Q12), and stay as cheap
  under a `lazy` rest route. An `unrecorded` one keeps scanners' probes out of the log too.

`ChannelRead` is the same kind of thing (a subscription changes nothing), and a page's
subscription route could be `lazy` by the same argument. That is fork 23, not taken here.

#### 5.11.6 The checklist (§4)

1. **The item and its words:** `FileServe("site")`, from `site.serve(…)` (or a `file_serve`
   node): "answer its callers with files from the folder \"site\"". Per folder, the grain
   `FileRead` already has.
2. **The resource:** the folder an admin binds, as for `FileRead`. A deploy is refused until it
   is bound (named-folders §2). No credentials.
3. **Reads are the run's input:** the handle is a logged read (§5.11.4). The bytes sent are not
   an input; they are checked against it.
4. **Effects land when the run finishes, at most once:** the answer goes out after the run
   commits. A run that runs again (claims) answers once, from its last attempt. A file that
   changed is a 503, never different bytes.
5. **Sharing:** nothing new. A run that writes the file it serves serves the handle it read, so
   the write isn't visible to this caller, and the send then fails the hash check (503) if the
   write landed first.
6. **Triggers:** none; it answers a route.
7. **Secrets:** a file isn't a secret, and `taint.rs` sees no secret in a handle. A flow that
   writes a secret-derived text to a folder is already refused (named folders), so serving can't
   carry one out.
8. **Limits:** `max_served_bytes` (§5.11.5), with a default and a ceiling in the policy, shown in
   the deploy bar.
9. **Verify:** recomputes the handle and the answer; the bytes are checked by hash (§5.11.4).
10. **Seeing it:** `holon check`'s words, the deploy bar, the manual's permissions page and its
    files guide, and the run log's line (§5.11.4).
11. **The sample:** `site`, deployed by the e2e tests against a fixture folder (§5.11.7). No
    outside service is involved.
12. **The engine:** none new. hyper's streaming body, as subscriptions use.

#### 5.11.7 Samples

- **`site`** (the example above): a folder bound to a built app, its API beside it, and a
  history-mode fallback to `index.html`. The e2e test binds the folder to a fixture `dist/` (an
  `index.html`, a script, a PNG) and checks a deep link gets the page, the script its type, the PNG
  its exact bytes, a second request a 304, and a missing asset 404 under `/assets/`.
- `log-archive` gains `GET /logs/:name/download` answering with the whole log: the binary-safe,
  any-size counterpart of its `tail`.

#### 5.11.8 Slices

Step 1.8 comes after 1.6 and before 1.9 (§7, revised 2026-11-09), the rest segment first:

1. **1.8a, the rest segment:** core (`Segment::Rest`), the language's route path and the printer,
   the bridge's `route_path`, the editor's `parsePath` and matcher, docs; the `site` sample
   without `serve` (an index page from a `js` holon, a catch-all answering it), run live.
2. **1.8b, `file_serve`:** the primitive, the capability and its description, `respond`, the
   transport's file answer, small files from memory, big ones streamed and checked, ETag and 304,
   the log line, `FileServe` allowed `lazy` and `unrecorded` (`is_answer`, §5.11.5); the `site`
   sample in full, `log-archive`'s download.
3. **Later:** `HEAD`; `Range` (206, for video and resumable downloads, checked against the whole
   file's hash with the BLAKE3 tree proofs ranges.rs already makes); precompressed siblings
   (`app.js.br` served for `app.js` when the caller accepts it, each file checked by its own
   hash); the handle cache.

#### 5.11.9 Refused, with the reason

| Refused | Why |
|---|---|
| A static mount by host path (`static "/var/www"`) | The flow would see a path, the version wouldn't cover what it serves, and files could change under a fixed hash with nothing recorded. Folders are bound by an admin and read by hash. |
| Recognizing a plain handle record in `respond` as a file | A flow answering `respond inbox.file(n)` today sends the handle as JSON. Changing that would change existing flows, and a record isn't a permission. `serve` is explicit and asks `FileServe`. |
| A redirect to a backend's presigned URL | The host would never see the bytes, so nothing stands behind the hash in the log. |
| Serving without a run | It would make a request the log can't account for, even as a count. `lazy` is the cheap recorded path, `unrecorded` the approved unlogged one (both still runs, both counted); a CDN in front is the fast one. |

#### 5.11.10 Forks

- **Rest matching zero segments** (`/*path` also matching `/`) would save the second route in
  the sample. Refused for now for consistency with `:param`, which is never empty. Revisit if
  every sample needs both.
- **A folder bound as `--no-serve`**, so an admin can keep a folder from ever being answered with
  whatever the flow asks. The deploy's approval already shows `FileServe`; add this if a team
  asks.
- **Content types:** core's table covers the web's types (html, css, js, mjs, json, map, svg,
  png, jpg, gif, webp, avif, ico, woff, woff2, ttf, wasm, txt, pdf, mp4, webm, mp3). Anything
  else is `application/octet-stream` unless the flow sets one.
- **`FileServe` recording lightly, and `ChannelRead`:** §8, forks 22 and 23.
- **A flow's own files, bundled with its version,** instead of a folder: §5.13.

#### 5.11.11 What stays true

- **Purity:** `file_serve` is pure: a handle in, an effect value out, the type from a table in
  core. The host reads the bytes after the run, as it streams a subscription.
- **Content addressing:** the rest segment is part of the endpoint's `path` param, so of its hash;
  no unit that exists today changes.
- **The effect boundary:** the answer is an `Effect` the host performs, never a value a node
  reads.
- **Derived labels:** `FileServe(folder)` is read from the node's param, not declared; the route
  list shows the rest segment.
- **The record:** every request is still a run, recorded at its route's level (request-path C5);
  where it is recorded, what was sent is recorded by hash; a caller never gets bytes other than
  the handle's hash.
- **Request-path I-g and I-h:** `FileServe` changes nothing, so allowing it `lazy` and
  `unrecorded` keeps "nothing that changes anything goes unrecorded or lazy"; `unrecorded` is
  still a person's approval.

#### 5.11.12 Slice 1.8a as designed (2026-11-09, against `5e4d472`)

§5.11.2 stands. What the code at `5e4d472` changes about where it is built:

- **The language has no route-path tokens any more.** A route is `route.get("/*path", (req) =>
  …)`: its path is a string, lowered as the request node's `path` param as written
  (`lower.rs`, `route`), checked by core's `parse_path` when the routes are read
  (`routes.rs`, `find_routes`, and `endpoint_unit`). `Tok::Star` and a printer change aren't
  needed: the printer writes the string back. What the language does change is its words: the
  error for a bad path (`use /todos, /todos/:id or /files/*path`), `Path`'s doc in `holon.d.ts`,
  the editor's help for `route` (`editing.rs`) and the agent's (`agent.rs`).
- **The editor asks core instead of keeping a copy.** `endpoint.ts` matches requests itself for
  the request tester (`matchRoute`, `requestRecord`), a second copy of `match_route`. Instead of a
  test keeping the copy equal, core gets `pick_path(paths, request_path)`, which `match_route` is
  built on, and the bridge hands it to the editor as `route_pick(paths, request_path)`: `{index,
  params}` (params as the request wrote them) or `null`. The editor keeps only what is its own:
  which endpoints have the method, decoding, the 405. `route_path` gains `rest` beside `literal`
  and `param`.
- **Clashes are core's too:** `same_requests(a, b)`, whether two paths match exactly the same
  requests, used by `read_server` and `routes.rs` (each has its own `shape` today).

**Core** (`declare.rs`):

```rust
pub enum Segment { Literal(String), Param(String), Rest(String) }
```

- `parse_path`: a part `*name` (a word, the names shared with params, so `/:path/*path` is bad) is
  `Rest(name)`, and only as the last part: `/a/*p/b` and `/*p/*q` are bad paths.
- `pick_path`: a rest path matches when the parts before its rest match as today and one or more
  parts follow, each one *plain*: not empty, not `.` or `..` once `%2e` is read as `.`, and with
  no `%2f`, `%5c` or `\` in it, which would decode to a separator. A part that isn't plain makes
  the path not match, so `/docs/a%2Fb` and `/docs/..` get a 404 from `GET /docs/*page`, as from
  no route at all. Besides §5.11.2's `/`, `\` is refused because a folder on Windows reads it as
  one. The rest's param is the parts joined by `/`, as written; the host decodes it as it decodes
  every param (`data.rs`, `percent_decode`), so `css/site%20a.css` reaches the flow as
  `css/site a.css`, and the editor decodes it as it does every param.
- **Which wins:** a path without a rest beats any path with one, then the most literal parts, then
  the earlier path, as §5.11.2 says.
- `same_requests`: equal parts, every param alike, a rest only alike a rest. `/a/*x` and `/a/*y`
  clash; `/a/*x` and `/a/:x` don't.

**The tests' fillers.** The eval and server goldens (`golden.rs`, `tests.rs`) fill a route's
`:name` with `tea` to send it a request; they fill `*name` the same way. `load.rs` already fills
and matches `*rest`.

**The sample `site`** (port 8196, 8131 being taken): the API and an app's page on one port.

```ts
route.get("/api/hello", () => ({ hello: "world" }))
route.get("/api/*rest", (req) => status(404, { error: `no API at /api/${req.params.rest}` }))
route.get("/", () => respond(page(""), { headers: html }), { log: "lazy" })
route.get("/*path", (req) => respond(page(req.params.path), { headers: html }), { log: "lazy" })
```

`page` is a `js` holon writing the app's one page: where it was opened (escaped), and a script
that reads `/api/hello`. A deep link (`/docs/setup`) gets the page, which says where it is; an
unknown API path gets a JSON 404, not the page, since `/api/*rest` is the longer prefix; and
`/api/hello` beats both. 1.8b replaces `page` with the folder's `index.html` and the files beside
it.

**Tests.** Core: the paths parsed and refused, a table of requests against paths (which wins, the
params, the parts that aren't plain), the clashes. The host: `data.rs`'s params decoded and its
405 over rest routes; a server test deploying `site` and asking a deep link, `/`, an unknown API
path, `/api/hello`, `..` and `%2F`. The editor: `endpoint.test.ts` over the same requests through
the bridge; `samples.test.ts` builds the sample. e2e: the sample deployed in the flows page's
loop, and a test of its answers.

**Docs:** `LANGUAGE.md` (routes), the manual's reference (routes, the most specific route), and
this L4's as built.

#### 5.11.13 Slice 1.8a as built (2026-11-09)

As designed (§5.11.12), with what building it found:

- **Core** has `Segment::Rest`, `pick_path` (the one place a request is matched: `match_route`
  picks among its method's routes with it, and the bridge's `route_pick` among the paths it is
  given) and `same_requests` (the one place two routes clash). The closeness of a match is
  `(no rest, literals)`, compared as a pair, so a route without a rest always comes first.
- **The editor matches nothing itself any more.** `matchRoute` and `requestRecord` ask
  `route_pick`, and keep only the method's filter, decoding and the 405; `parsePath` reads `rest`
  beside `literal` and `param`. A path core refuses matches nothing there, as on the server.
- **What a client does with dots.** A browser, `fetch` and Playwright's request read `..` and
  `%2e%2e` as the path's parent before they send, so they never ask `/docs/..`; `curl
  --path-as-is` does. The server test and the live run ask it (404); the e2e test asks only what a
  client sends as written (`%2F`, `%5C`, an empty segment).
- **The goldens** fill `*name` with `tea` as they fill `:name`. The `site` sample comes after
  `guestbook` in the samples' order, so the server golden's runs of every sample after it have
  later clocks and nonces; their answers are unchanged.
- **Run live** on a scratch server of the slice's build on loopback, the sample deployed with
  `holon deploy --approve`: `/api/hello` answered `{"hello":"world"}`; `/api/nope/more` 404 with
  `{"error":"no API at /api/nope/more"}`; `/`, `/docs/setup` and `/docs/a%20guide/part%202?x=1`
  the page, opened at `/`, `/docs/setup` and `/docs/a guide/part 2`; `/%3Cb%3Ehi` the page with
  `&lt;b&gt;hi`; `/docs/a%2Fb`, `/docs/a%5Cb`, `/docs//a`, `/docs/`, `/api/%2e%2e` and `/docs/..`
  no route's (404); `POST /docs/setup` 405 with `allow: GET`. In a browser, the deep link opened
  the page, its script got the API's answer, and its links went from page to page. The 19 runs
  export and verify: the 12 that ran re-run to what their entries say, the 7 no route took are
  logged as such.

#### 5.11.14 Slice 1.8b as designed (2026-11-09, against `15d81b3`)

§5.11.3–§5.11.7 stand, with what the code at `15d81b3` changes:

- **No log-level amendment.** §5.11.5 has `may_record_lightly` take `FileServe` by a new
  `is_answer()`. That rule is gone: since request-path C5's revision a handler's level is what its
  author declares, whatever it does (`declare.rs`, `LogLevel`), and the approval says what an
  unrecorded one leaves out. A file route is `lazy` or `unrecorded` as any route is; nothing to
  amend, and fork 22 is moot.
- **The record is written before the bytes go.** A strict run's record is on disk before it
  answers (C2 e), so the checks that decide the status run before the record, in the host's
  answer to the run (`listeners.rs`, `serve_flow`), and the record says what was sent: 200, 304 or
  503. Only a big file's last check, its hash at the end of the stream, comes after; a mismatch
  cuts the connection and says so on the deployment's timeline (`… changed while it was sent: the
  connection was cut`), since the record can't be changed after it is written.
- **A status the flow chose stays.** `return status(404, site.serve("404.html"))` answers the
  page with a 404; only a 200 is answered 304 to a matching `If-None-Match`.
- **A missing asset is a route of its own.** §5.11.7's "a missing asset 404 under `/assets/`" needs
  a route without the `index.html` fallback: `GET /assets/*file`, the longer prefix (§5.11.2), so
  a script that isn't there is a 404, never the page served as a script.

**Core.**

- **`file_serve`**, a genesis leaf: input `file` (a handle, `Record`), param `folder`, output an
  Effect. Pure: from a handle of its folder (`{folder, name, size, hash, modified}`, each of its
  type) it makes `Effect { sink: "file_serve:site", payload: {folder, name, size, hash, modified,
  type} }`; from anything else, nothing. `type` is `content_type(name)`, a table in core by the
  name's extension (§5.11.10's list, `text/…` with `; charset=utf-8`), else
  `application/octet-stream`. Not a sink the host performs and not triggered: like
  `channel_subscribe`, what a route answers with.
- **`FileServe(folder)`**, a capability: `Sink`, not sourced, from `file_serve`'s `folder`;
  described "answer its callers with files from the folder \"site\"". The handle's `FileRead`
  stays the handle leaf's. `firing.rs` never performs it (as `ChannelRead`); `touches.rs`, the
  bridge's `WCapability`, `restore.rs` and the editor's holdings (a folder's use `serve`) learn it.

**The language.** `site.serve(name)`, on a folder the flow imports, lowers to
`file_serve{folder: site}(file_handle{folder: site}(name))`; it takes a name, as `site.file(name)`
does. It is an answer: `return site.serve(n)`, `respond(site.serve(n), { headers })`,
`status(404, site.serve("404.html"))`, and `site.serve(a) ?? site.serve("index.html")` (the lazy
evaluator reads the second handle only when the first file isn't there). The printer prints the
pair back as `site.serve(name)` when the handle is the serve's alone, else each as written.
`holon.d.ts`: `Folder.serve(name: string): Answer`.

**The host.**

- **`respond`** (`data.rs`) reads a `file_serve` effect as it reads a subscription, alone or as an
  envelope's body: `DataResponse.file: Some(Served {folder, name, size, hash, modified, type})`,
  the envelope's status and headers kept.
- **`serve_flow`**, after the run and before its record:
  1. `FileServe(folder)` granted, else 500 (as `ChannelRead`).
  2. The size within `max_served_bytes`, else 500 saying so.
  3. A 200 whose request's `If-None-Match` names `"<hash>"` (or is `*`): 304, no body.
  4. Small (up to `inline_file_bytes`): read through the deployment's folders, hashed, sent from
     memory if it is the handle's. Big: opened, its size and modified time the handle's, then
     streamed by the transport. Anything else (gone, changed): 503, `retry-after: 1`, its error
     `changed: site/x changed since the run read it`.
  5. Headers: `content-type` the payload's `type` unless the flow set one, `etag: "<hash>"`,
     `cache-control: no-cache` unless the flow set one. The length is the transport's.
  6. The record's `response_body`: `file site/assets/app.js (148.2 KB, blake3 7c1e2a9b…)`.
- **The transport** (`dataplane.rs`): `Answer::File`, the file read on the blocking pool in 64 KiB
  chunks into a channel, hashed as it goes, its length said; a hash that isn't the handle's at the
  end ends the body with an error, so the connection is cut before its last chunk. It holds a
  connection as a subscription does (`max_connections`).
- **The control plane's "send live"** gets a small file's bytes as any body, and a big one's line
  (`file site/x (…)`): the bytes go out on the deployment's own port.
- **`max_served_bytes`**, a file limit: 1 GiB by default, 1 TiB at most, set as any limit
  (`limits`, the deploy bar).

**Samples.** `site` in full: the folder `site` (read-only, a built app), `GET /api/hello`, `GET
/api/*rest` (a JSON 404), `GET /assets/*file` (`site.serve(`assets/${file}`)`, 404 when missing,
cached a year as its names carry hashes), `GET /` (`index.html`) and `GET /*path`
(`site.serve(path) ?? site.serve("index.html")`), the page routes `lazy`. `log-archive` gains `GET
/logs/:name/download`, the whole log as an attachment.

**Tests.** Core: `file_serve` on handles of its folder and others, the content types, the
capability and its words. The language: lowering, printing back, the errors. The host: `respond`'s
`file`; a server test of `site` against a folder: a deep link, the script's type, a PNG's exact
bytes, a 304, a missing asset's 404, a file changed between its handle and its send (503), a big
file streamed (through the data plane) and cut when changed while sent, a file past
`max_served_bytes`, a deployment not granted `FileServe`. The goldens: `site` reads a folder now,
so the server golden leaves it out. e2e: the folder bound to a fixture `dist/`, the sample
deployed, a deep link, the script, the PNG's bytes, a 304, a missing asset; `log-archive`'s
download.

**Docs:** the files guide (serving a folder), the permissions page, `LANGUAGE.md`, the manual's
reference and leaves, and this L4's as built.

#### 5.11.15 Slice 1.8b as built (2026-11-09)

As designed (§5.11.14), with what building it found:

- **Core** has `file_serve` (genesis 193), `content_type` (exported, so the host and a later
  `Content-Type` check read one table) and `FileServe`, in the saturated set too (22). A handle is
  taken only whole: its five fields, each of its type, the hash BLAKE3's lowercase hex, and nothing
  more, so a record a flow builds that isn't one gives no answer.
- **The language** lowers `site.serve(name)` on the folder's own path, before the method table, and
  takes exactly one name; the printer prints the pair back when the handle is the serve's alone
  (`inline_source`), else `file_serve(…)` by name. It is an expression, as `site.file(name)` is, so
  `??`, `status(…)` and `respond(…)` take it with no rule of their own.
- **A name no file can have names no file.** A route's path is the caller's: `/Talk:Page` or
  `/docs/con` handed to `site.serve` made the run fail (a name with a `:`, or Windows' `CON`, is
  `bad_file_name`'s) and answer 500 where the page was meant. A read of such a name (`existing`)
  now has no value, so `??` falls back; a name that would lead out of its folder (a `..` part, a
  leading `/`, a `\`) still fails the run, as the named-folders note has it. A folder's own name is
  no file either (`open_file`): Windows won't open one as a file, so `/assets` answered 500 there
  and nothing on Linux.
- **The checks are `listeners::serve_file`,** run after the run's other checks and before its
  record, its error the entry's (`changed: …`, `refused: …`). A small file is read and hashed there
  and sent as any body; a big one is opened and checked by size and time, and handed to the
  transport open (`Served::opened`), so what is streamed is the file checked.
- **`holon verify` expects the host's status,** not the flow's, for a file: `served_status` is the
  one rule, shared by the host and verify, from the flow's status, the request's `If-None-Match`
  and the entry's error. Without it a 304 re-ran as the flow's 200 and differed.
- **The transport** (`Answer::File`) holds back the last 64 KiB piece until the whole is the
  handle's, says its length (`SizeHint::with_exact`), and ends a changed file with a body error, so
  hyper cuts the connection short of its `Content-Length`. The body's error type is `io::Error`
  now; the other answers can't fail.
- **The record** says `file site/assets/app.4f2a.js (19 bytes, blake3 9a1c…)`, sizes as the folder
  guide writes them (`size_text`, KiB); the control plane's "send live" gets a big file's line,
  a small one's bytes.
- **The editor** has the leaf in the files group of the palette, `FileServe` among the folder uses
  (`serves its files`), and `max_served_bytes` in the deploy bar. The permissions page gains the
  file limits, which it hadn't listed.
- **Tested:** core (the effect from a handle and not from anything else, the types, the
  capability), the language (lowering, the errors, printing back), `respond`'s file, the `site`
  server test (§5.11.14's list, through a real socket for the big file, its cut on the timeline),
  the goldens (`site` leaves the server golden; the evaluator's gains the two routes), vitest,
  wasm, and e2e: `site` against a fixture build, a deep link opening in a browser, its script
  running; `log-archive`'s download, 2.5 MB, so streamed.
- **Run live** on a scratch server of the slice's build: §5.11.16.

#### 5.11.16 Slice 1.8b run live (2026-11-09)

A scratch server of the slice's release build, `site` bound read-only to a fixture build (its
page, `assets/app.4f2a.js`, an 11-byte PNG, a 5.8 MB `assets/big.txt`), `logs` and `archive` for
`log-archive`, both samples deployed as written (8196, 8124), approved with their asks
(`FileServe("site")`, `FileServe("logs")` beside the reads):

- **A deep link** (`/docs/a%20guide/part%202`) answers the page: 200, `text/html; charset=utf-8`,
  its `etag` its BLAKE3 hash, `no-cache`, 178 bytes; `/` the same. In the browser pane
  `/docs/setup` opened the app there, its script ran ("Opened at /docs/setup") and asked its API
  ("The API says hello, world."), with nothing on the console.
- **The script** is `text/javascript; charset=utf-8` with the route's `immutable` cache-control;
  the PNG's bytes arrive as written, `image/png`.
- **The browser's copy:** `If-None-Match` with the etag a 304 and no body; a stale one a 200.
- **A missing asset** a 404 `{"error":"not found"}`, never the page; `/api/nope` the API's own
  JSON 404.
- **Names no file can have:** `/Talk:Page`, `/docs/con` and `/assets` (a folder) the page, by
  `??`; `/%2e%2e/secret` matches no route: a 404, recorded with nothing run.
- **The big file** streamed: 200, `content-length: 5800000`, the bytes the file's. Sent again at
  400 KB/s with its last 8 bytes changed two seconds in, the connection was cut at 5,439,488
  bytes (curl: a partial transfer), and the timeline said "site/assets/big.txt changed while it
  was sent: the connection was cut".
- **`log-archive`'s download:** `attachment; filename="app.log"`, 2,508,890 bytes as written; a
  log that isn't there a 404.
- **Exported and verified:** `site`'s 21 runs re-ran but one (the `..` path, where nothing ran),
  the 304 among them, each what its entry says; the record kept `file site/assets/big.txt
  (5.5 MiB, blake3 9538ba51…)`, never the bytes. `log-archive`'s 2 runs re-ran too.

Not seen live: the 503 for a file changed between the run and its send, which needs the change to
land inside a run's few milliseconds; the server test makes it by calling `serve_file` with a
handle the file no longer has.

### 5.12 Inbound WebSockets: a connection whose every message is a run

Designed 2026-11-04 against `450e3de` and re-checked the same day against `ff03a80`, cited by file
and line there. Step 1.9. Until that day inbound WebSockets were "later" here (§5's table, the end
of §5.9, Tier 3): "channels (SSE) and a POST cover what a browser sends and hears". Decided then
that they are fundamental: the libraries web apps are built on (collaborative editing, GraphQL
subscriptions, presence) expect one two-way connection, not two half-connections.

It follows §5.9's rules for every connection that outlives a run: **the host owns the connection;
a run sees whole messages, never bytes off the wire; what a run received and sent is its input,
logged; the connection's own life is outside every run, in its own record**. §5.12.9 answers §4's
checklist. It builds on [channels](channels-L4.md) (packs to subscribers),
[triggers](triggers-L4.md) (a flow with several handlers) and the request path's log levels (C5).

**Names.** §5.9 already uses *socket* for a bound connection to a payment network
(`holon socket add`, `SocketUse("processor")`, `on message processor (m)`). So this is
`websocket`, its item `WebSocket(path)`, and its handlers are blocks inside the statement, never a
top-level `on message`.

A flow declares a **websocket**: a path a browser opens a WebSocket on. What the flow does
happens in runs, as for requests:

- **open** is a run when a browser connects. It decides whether to accept, and what the
  connection hears (channel subscriptions).
- **message** is a run per message the browser sends. Its `respond` goes back to that connection
  only.
- **close** is a run when the connection ends, whichever side ends it.

Nothing goes out on a connection except a run's reply or a pack on a channel it subscribed to:
both are recorded effects of recorded runs. So a connection's whole conversation is in the log,
and every run in it replays as a request's does.

```
flow room
port 8132
channel room
collection people

websocket /room/:name {
  open (req) {
    people[req.conn] = { name: req.params.name, since: now }
    room.send({ joined: req.params.name })
    respond room.subscribe()
  }
  message (msg) {
    who = people[msg.conn]
    room.send({ from: who.name, text: msg.text })
  }
  close (c) {
    delete people[c.conn]
    room.send({ left: c.params.name })
  }
}
```

`check` says:

```
a server on :8132
  WebSocket /room/:name
it may:
  read the clock
  hold WebSocket connections on /room/:name, and run on each message
  read items of its collection "people"
  put and delete items of its collection "people"
  send packs on its channel "room" to whoever subscribes
  let callers subscribe to its channel "room"
```

#### 5.12.1 What is already there

- **Channels** (built) are the push half: a flow sends packs from any run, the host delivers them
  to subscribers after the run commits, keeps the last `channel_keep`, drops a slow subscriber,
  and caps subscribers per deployment (`channels.rs` 39–50, 164). Today the only subscriber is a
  server-sent-events response.
- **Routes** run once per request (`listeners.rs`, `serve_data`) under the deployment's grant,
  with claims ordering runs that touch the same keys. Every request is logged (255). A reply that
  carries a secret needs `SecretAnswer` (`listeners.rs` 188–196).
- **Each handler has a log level** (request-path C5, built): `strict` by default, its record on
  disk before its answer (about 0.6 ms of fsync, with group commit, C2 e); `lazy`, on disk within
  about a second; `unrecorded`, counted and never logged, failed or not (resilience-audit §2.8),
  approved by a person. Only a handler that may record lightly takes a word: every capability
  sourced, none `Await` (`may_record_lightly`, `declare.rs` 86).
- **What starts a run** is one item per kind (§1): a route `NetworkIngress`, a clock `Clock`, a
  file `FileArrival`, an action `Action`.
- **The data plane** is hyper http1 on tokio (`dataplane.rs`), each connection served by
  `serve_connection` (162) without upgrades. hyper 1 supports them (`.with_upgrades()`,
  `hyper::upgrade::on`). No WebSocket codec is in the tree (`Cargo.lock` has no `tungstenite`);
  §5.9 names tokio-tungstenite for outbound feeds (step 1.6), so both directions share one engine.
- **There is no bytes value** (§1). One is planned, `Bytes` (§5.10, step 1.1).
- **Browsers' `WebSocket`** can't set headers: a page authenticates with a cookie or a token in
  the query string, both of which the opening request carries.

#### 5.12.2 The connection and its runs

- **The opening request** is a GET with `Upgrade: websocket`. It matches the websocket's path like
  a route (`:params`, and `*rest` from §5.11.2), and runs **open** with the request record plus
  `conn`, the connection's id (random, chosen by the host, so it is in the record and the log).
- **Open's answer decides the handshake.** A subscription, or a list of them, accepts and
  subscribes. A record with `status` 4xx or 5xx refuses the upgrade with that status. Absent
  (`missing:`) is 403. Any other value accepts and is sent as the first message. The 101 goes out
  once open's run has committed, so its writes land before the first message is read.
- **Each text frame** runs **message** with `{conn, params, query, text}`, where `json` is
  `parse_json(text)`, lazily, as `req.json` is. A message's `respond` goes to that connection
  only; no `respond` sends nothing.
- **Messages on one connection run one at a time, in order.** The host queues them per
  connection, up to `socket_queue` (32). A full queue closes the connection with 1013 ("try again
  later"). Messages on different connections run concurrently, as requests do, and claims order
  them where they touch the same keys.
- **Packs** on a subscribed channel go out as text frames, each a pack's JSON, after their run
  commits, as for server-sent events. They interleave with replies in the order the host sends
  them.
- **close** runs once, after the last message's run, with `{conn, params, query, code, reason}`.
  It answers nobody.
- **Pausing or stopping** a deployment closes its connections with 1001, and each runs close.
  Redeploying doesn't close them: the next message runs the new version, as the next request
  would.

#### 5.12.3 The connection's own record

§5.9 and fork 20: what isn't a run has its own record. Per connection, a log hash-chained like a
deployment's and kept as long as its runs: opened (when, from where, the open run's seq), pings
and missed pongs, frames refused (too big, binary before `Bytes`, over the rate), the queue full,
and closed (code, reason, by whom). It is synced as a `lazy` run's record is (request-path C2 d):
on disk within about a second. Pings are the host's (every 15 s; a connection that doesn't pong
within 30 s is closed with 1001), from nothing a run computed. `holon log --conn <id>` shows a
connection's runs and its record together, in order.

#### 5.12.4 The language and what it lowers to

`websocket PATH { open (r) {…} message (m) {…} close (c) {…} }`. `open` and `message` are
required, `close` is optional. Each block is a route's block: statements, `respond` (not in
`close`), its own names, and the same optional word before its braces, `lazy` or `unrecorded`
(`message (m) lazy { … }`), under the same rule (§5.12.5).

Each lowers to an endpoint-shaped composite with frozen params `declare = "websocket"`, `path`,
`on` (`open`, `message` or `close`), and `log` when it isn't strict, as a route's carries
`declare = "endpoint"`, `method`, `path` (`declare.rs` 1–24) and `log` (C5). `read_server`
collects them into `Server.websockets`, by path, each with its handlers' hashes. A websocket's
path clashes with a GET route of the same shape: a path is either a page or a connection.

#### 5.12.5 Consent

- **The item is the trigger:** `WebSocket("/room/:name")`, "hold WebSocket connections on
  /room/:name, and run on each message". Every kind of thing that starts runs has its own item
  (§1), and this one differs from a route in a way an approver should read: callers stay
  connected, and their messages start runs for as long as they do.
- Subscribing is `ChannelRead`, sending `ChannelSend`, as now. A reply carrying a secret needs
  `SecretAnswer`, as a route's does.
- **Log levels per block,** C5's words and rule unchanged. A block that only reads and replies
  may be `lazy` or, approved, `unrecorded` (labelled `WebSocket /room/:name message`, as the log
  and the tallies name it). One that sends packs or writes state is `strict`: its record is on
  disk before its reply and its packs go out. So is an `open` that subscribes, as long as
  `ChannelRead` isn't allowed to record lightly (fork 23).
- **Limits** a deploy asks for above their defaults, up to the host's ceilings, as
  `run_budget_ms` is: `max_websockets` per deployment (100), `max_message_bytes` (64 KiB),
  `messages_per_second` per connection (10, bursts of 20; past it, 1008).

#### 5.12.6 The log, replay and verify

- **Every run is a log entry at its block's level,** `kind` `open`, `message` or `close`,
  `route` `WebSocket /room/:name`, with `conn` in its request record. An `unrecorded` block's runs
  are counted in its tally instead. A reply is the entry's `response_body`; a pack is in its
  sending run's `performed`, as now.
- **Replay and verify** are a request's: the record, the state as the run began, the clock, the
  answers. A connection's order is the order of its entries.
- **Volume is the price:** a chat at ten messages a second writes ten entries a second, unless a
  person approved its block `unrecorded`.

#### 5.12.7 Limits that stay

- **Latency per message is a run's.** A `strict` block (any that sends packs or writes) waits
  for its record's fsync, about 0.6 ms, shared with other runs by group commit (request-path
  C2 e); a `lazy` one costs tens of microseconds over its run. One connection's messages run one
  at a time, so a strict block takes at most about 1,600 messages a second from one connection.
  Fine for chat, presence, notifications and collaborative edits at typing speed. Not for 60 Hz
  game state or cursor streams; those want the browser to batch.
- **No memory per connection except state.** What a connection is (who signed in, which room) is
  kept in a collection or state keyed by `conn`, so it is recorded. There is no hidden
  per-connection variable, by design.
- **Inbound messages are at most once.** A message whose run hadn't committed when the server
  stopped is gone with the connection. The client sees the close and resends, as it must over any
  WebSocket.
- **One machine:** a connection lives on the machine it opened on, like everything in Holon today.

#### 5.12.8 Binary frames

Refused (1003) until `Bytes` lands (§5.10, step 1.1). Then a binary frame is `msg.bytes`, a
`Bytes` value within `max_message_bytes`, logged as §5.10's boundary rules say (base64 in the log
line, large ones in `blobs.rs`), and a reply or pack that is `Bytes` goes out as a binary frame.
Nothing here needs a second representation of binary data.

#### 5.12.9 The checklist (§4)

1. **The item and its words:** `WebSocket(path)` (§5.12.5).
2. **The resource:** none bound; the flow's own port, as routes. No credentials.
3. **Reads are the run's input:** each message is its run's request record, logged (§5.12.6).
4. **Effects land when the run finishes, at most once:** a reply and packs go out after the
   commit, and after the record is on disk for a `strict` block. A run that runs again (claims)
   sends its reply once, from its last attempt, and its packs once, as channels already do.
5. **Sharing:** claims and locks, as for requests. A connection lives on one server.
6. **Triggers:** a message is whole when its last frame arrives (the settle rule). It isn't run
   twice: one connection's queue hands each message to one run (the key is `conn` and the
   message's place in it). After the run, nothing: it isn't acknowledged to the client unless the
   flow replies.
7. **Secrets:** a reply as a route's answer (`SecretAnswer`); a pack as channels rule (refused
   when computed from a secret).
8. **Limits:** §5.12.5, each with a default and a ceiling, shown in the deploy bar.
9. **Verify:** re-runs every open, message and close run; the connection's record is checked by
   its chain, not re-run.
10. **Seeing it:** `holon check`'s words; the deploy bar; the deployment's page showing open
    connections, their age and their queue; the manual's permissions page and a guide.
11. **The sample:** `room` (§5.12.11), deployed by the e2e tests with the browser as the client.
    No outside service is involved, so no stand-in is needed.
12. **The engine:** tokio-tungstenite on the data plane's runtime, shared with step 1.6's
    outbound feeds. Pure Rust, rustls where TLS is involved (it isn't here: TLS ends at the
    ingress).

#### 5.12.10 Forks

1. **Subprotocols** (`Sec-WebSocket-Protocol`, which GraphQL over WebSocket and some sync
   libraries use): open's answer names one from the request's list. *Lean:* in the first slice
   if a target library needs it, otherwise the second.
2. **Close with a reason from the flow:** `respond close(4001, "kicked")` from a message run.
   *Lean:* with targeted packs (§5.12.11, slice 2).
3. **Rate limits per caller instead of per connection,** since a client can open many.
   *Lean:* `max_websockets` bounds that for now.

#### 5.12.11 Samples and slices

1. **Text frames:** the statement, lowering, `read_server`, the item, the data plane,
   open/message/close, replies, subscriptions, limits, the log and the connection's record; the
   **`room`** sample (above, with a page at `GET /`, a `js` holon whose script opens the
   connection). The e2e test opens two connections from the browser, sends from one, sees it on
   the other, and closes one to see `left`.
2. **Targeted packs:** `room.send(v, to: conn)`, a pack only the subscriber on that connection
   gets. Direct messages, and answering one connection from another's run or from a clock.
3. **Binary frames**, once `Bytes` lands (§5.12.8).

#### 5.12.12 Refused, with the reason

| Refused | Why |
|---|---|
| A connection the flow writes to directly (`conn.send(x)` from anywhere) | Anything sent must be a recorded effect. A reply (this connection) and a pack (subscribers, or one connection in slice 2) are those effects. A connection handle in a value would also outlive its run. |
| Per-connection memory in the engine | Hidden state a replay can't see. State keyed by `conn` is recorded. |
| Running one connection's messages out of order | A conversation must replay in the order it happened. |
| Messages answered by the host without a run (an echo) | Every message in is a run, so the log holds the whole conversation, except where a person approved a block `unrecorded`, and then the tally counts it. |
| `socket` as the statement's name | Taken by §5.9's bound sockets. |

#### 5.12.13 What stays true

- **Purity:** runs are pure as any route's; the connection is the host's.
- **The effect boundary:** replies and packs are effects the host performs after the run; nothing
  in a run touches the connection.
- **Derived labels:** `WebSocket(path)` is read from the server's children, not declared.
- **The record:** every message in is a run, recorded at its block's level (request-path C5);
  everything out is a reply or pack of such a run, or the connection's own record; one
  connection's runs are in order.
- **Request-path I-g and I-h:** a block that sends or writes is `strict`; `unrecorded` is a
  person's approval.

### 5.13 What else a web app asks for

Asked for while weighing what it takes for a flow to host a React app (2026-11-04). None is
scheduled; each is sorted by §2's ways, and gets a design here when a step takes it.

**Bodies a browser sends that aren't text.** A route refuses a body that isn't UTF-8, 400
(`data.rs` 118); a resume route reads it lossily (`listeners.rs` 152). §5.10 settles bytes Holon
sends; this is the other direction:

- **Small bodies** (within `max_body_bytes`) become `req.bytes`, a `Bytes` value, when the body
  isn't UTF-8 or its type isn't text or JSON, logged by §5.10's boundary rules. `req.body` stays
  text where it is. A resume route takes the same rule instead of reading lossily. Way: the value
  kind, step 1.1.
- **Big uploads** never enter a run (named-folders §4): the host stages the body in the blob
  store as it arrives (by hash, swept when no run names it), and the run gets a handle,
  `req.file`. Keeping it is the flow's own write, `inbox[name] = req.file`, a copy by handle when
  the run commits, under `FileWrite` as now. Way: no new item (the route's own port; the write is
  `FileWrite`).
- **`multipart/form-data`** (a browser's file form): a leaf over `Bytes` that splits it into parts
  (R1), each part a handle when the upload is staged.

**Cookies and sessions.** Reading is covered: `req.headers.cookie` is text, and a new `cookies`
leaf (R1) would parse it. Setting several cookies isn't: a response's headers are a record of
text values (`data.rs` 186), so `set-cookie` can't repeat. A header whose value is a list of
texts becomes one header line each. A session is then covered by what exists: a cookie signed
with `hmac_sha256` over a secret and checked with `secure_equals` (a one-way leaf, so `taint.rs`
lets it out), or a random id keyed into a collection. Way: covered, once headers take lists
(data plane, no item); a sample, `sign-in`.

**A Content-Security-Policy.** A flow sets one today, as any header. Holon can't derive one: the
manifest is the server's reach, and a page's reach is the browser's, which Holon neither sees nor
enforces. Its promises stop at the server, and its docs should say so where pages are served.
Way: covered (a header); later, a server feature: an operator's default for `text/html` answers
(`default-src 'self'`), the flow's own header winning.

**A flow's own files, bundled with its version.** Instead of a folder an admin binds, a flow
carries its built app: `assets site from "dist"`. When the flow is saved, each file is hashed and
stored by hash as modules are (staged as preimages, past the control plane's 8 MiB limit,
js-node-L4 §3), and the unit gets a frozen table `{path → hash, size, type}`, so the files are
covered by the flow's hash: changing one is a new version, and every run names the files it
served. `site.serve(path)` reads that table: pure, no `FileRead`, no `FileServe` (the files are
the flow, as its code is), shown in `check` as "serves 37 files of its own (2.1 MB)". Replay
needs nothing from the host's disk, and export ships the files by hash. Open: how the text names
them (a directory beside the `.holon` file, and what `fmt` and the editor do with it), and how
long old versions' files are kept. Way: not a capability; a unit feature, after step 1.8.

**React rendered on the server.** A `js` holon can run React's synchronous `renderToString`, with
the flow loading the data first (the loader pattern), if it can place a library by hash instead
of pasting it into its body: a library module beside the engine (js-node-L4). Streaming render
and Suspense need a run that answers before it ends, and server components that fetch while they
render hide effects from the graph: both are refused by the effect boundary, not missing. Way: a
module (R1), later.

### 5.14 A flow as a command-line app

Asked 2026-11-04: can a flow be a command a person types (`todo add "buy milk" --due fri`), and
if not, what's missing: a new `holon` command, or a host capability that pipes what is typed into
the flow? **Mostly a new command,** a client over actions, which exist; one small host change for
questions asked mid-run; and **no new capability**. Checked against `ff03a80`.

**What works today.** `holon action <slug> <name> --input JSON` runs one of a deployed flow's
actions through the server (`POST /trigger/<slug>/<name>`), prints its answer, and exits 1 when
it answers 400 or more (`opscmd.rs` 275, `action_lines` 310). The run is approved, recorded and
named by its caller, as any action's. That is a one-shot command already:

```
holon action todo add --input '{"title": "buy milk", "due": "fri"}'
```

**What a command-line app needs that isn't there:**

1. **Arguments.** The input is JSON; nothing maps `add "buy milk" --due fri` to a record, and
   there is no `--help`.
2. **A name of its own.** It is typed `holon action todo add`, not `todo add`.
3. **Output.** The answer is always printed as indented JSON, so a text answer comes out in
   quotes; the exit code is 0 or 1.
4. **Questions mid-run.** A run that waits only prints that it is "waiting for an approval or an
   event" (`action_lines`).
5. **Progress.** Nothing is printed until the run ends.
6. **Files where the person is.** A flow can't read the caller's working directory, by design (a
   flow names things; the host decides where they are), and stdin goes in only as the JSON input.
7. **A server.** `holon serve` must be running, the flow deployed and approved.

**What to add.** §2's fourth way, a client feature, plus one change to the language and the host:

- **`holon do <slug> <action> [args…]`** maps a command line onto the input record generically:
  `--due fri` is `{due: "fri"}`, a bare `--force` is `{force: true}`, the words without a flag
  are `args`, and `-` reads stdin into `stdin`, as text. No declaration is needed; the flow reads
  the fields it wants. A text answer prints as it is, anything else as JSON (`--json` for the
  whole envelope); the exit code is 0 below 400 and 1 at or above it, unless the answer is an
  envelope with a whole-number `exit`. `--help` lists the deployment's actions and the input
  fields each reads, which the graph shows (`field` nodes on the input).
- **`holon command add todo <slug>`** writes a shim to `~/.holon/bin` (a shell script, and a
  `.cmd` on Windows) that runs `holon do <slug> "$@"`, so the tool is `todo add "buy milk"`.
  `holon command list` and `remove` look after them. The shim names a deployment, never a host
  path, and runs as whoever runs it, under their token (`holon token`).
- **Files go through the client.** `@notes.txt` as an argument sends the file's text as that
  field; `-o out.csv` writes the answer to a file. The client is the person's own program on
  their own machine, so their disk is its to read. The flow still never sees a path, and the run
  records what it was given (by hash, past `log_inline_bytes`). Binary files wait for `Bytes`
  (§5.10), and big ones for an upload handle (§5.13).
- **Questions mid-run, through `await`.** The pieces exist: a run can answer before it parks,
  hand out `resume_token(name)`, and wait at `await name` until a route that `resumes name` is
  called (`LANGUAGE.md`, "approval: a durable run"; `durable.rs`, `claim_by_token` 597,
  `resume_parked` 618). What's missing is a way to resume that isn't a route on the network, and
  a client that does it. So an **action may resume a waiting point**, `action confirm (input)
  resumes confirm { … }`, reached only through Holon, as every action is; and when a run started
  by `holon do` answers and waits, the client shows the answer as the question, reads a line,
  and runs that action with it and the token. The flow:

  ```
  action wipe (input) {
    respond { ask: "Delete {length(notes.list())} notes? [y/N]", token: resume_token(confirm) }
    yes = await confirm, expires: 5m
    …
  }

  action confirm (input) resumes confirm {
    respond "ok"
  }
  ```

- **Progress** reuses what a streamed reply already does (channels' reply, §5.9): `holon do`
  prints the pieces as they come.

**Piping what is typed into a run is not a capability.** Keystrokes or bytes fed into a running
run would break §5.9's rule that a run sees whole messages, and a run would no longer have one
input to replay from. The grain is **one line, one message, one run**, which is §5.12: a
`websocket` block whose `message` runs per line. `holon attach <slug> <path>` opens that
connection from the terminal, sends each line typed, and prints replies and packs as they come.
A REPL-shaped app then needs nothing beyond step 1.9 and that client. This is the opposite
direction to §5.7's terminals, where a flow drives an interactive program.

**What stays true.** Every command is an action's run or a websocket's message run: approved,
recorded at its handler's level, replayable. The client's reading and writing of the person's
files happens outside every run, by their own hand, and what a run was given is in its record.
Running a flow with no server at all is §5.15, `holon build`, which resolves fork 24.

**Slices.** (1) `holon do`, its argument mapping, output, exit codes and `--help`; `holon command`;
`@file` and `-o`; a sample, `todo-cli`, deployed by the e2e tests and driven by `holon do` in a
test. (2) Actions that resume, and `holon do` answering a question. (3) Progress, once actions'
replies stream. (4) `holon attach`, after step 1.9. Not scheduled in §7: a step when it is asked
for.

### 5.15 A flow as one executable: `holon build`

Designed 2026-11-07 against `490d03e` (`main`, rc.33), cited by file and line there. Asked the same
day: a command that turns any flow into a program a person runs alone, with no `holon serve`. It
resolves fork 24, "a flow run with no server", and takes the shape that fork leaned to: a
single-user host inside the binary. It builds on §5.14 (the command line an action reads), the
request path's log levels (C5), the run log and its witness (`recorder.rs`, `heads.rs`) and
exports (`verify.rs`).

`holon build todo-cli.holon.ts -o todo` writes one file: a copy of a holon runtime with the
flow's closure inside it (its units by hash, the text they came from, and the grant a person
approved), framed by a trailer. When `todo` starts and finds that payload, it is that flow and
nothing else: no editor, no control plane, no accounts, no other flows. `deno compile` and
`bun build --compile` work this way; what Holon adds is that the file carries what the flow may
do, who approved it, and a record of every run that `holon verify` checks anywhere.

```
$ holon build todo-cli.holon.ts -o todo
todo-cli: 9 nodes, 11 wires
a server of actions, run by hand, listening on no port
  add
  list
  done
unit 4be1…
it may:
  read the clock
  read items of its collection "todos"
  put and delete items of its collection "todos"
approve what it may do, as ada? [y/N] y
wrote todo: holon 0.1.0-rc.34 for x86_64-unknown-linux-gnu, and todo-cli (23 units)

$ ./todo add "buy milk" --due fri
todo-cli, built with holon 0.1.0-rc.34, approved by ada on 2026-11-07 (grant 3f9a1c…)
it may:
  read the clock
  read items of its collection "todos"
  put and delete items of its collection "todos"
its runs and data: /home/ada/.holon/apps/todo-cli
run it? [y/N] y
added buy milk

$ ./todo list
{"buy milk": {"due": "fri", "since": 1794067200}}
```

The person who runs it is asked once, on its first run (§5.15.4). Every run after that is an
action's run, approved, recorded and replayable, as on a server.

#### 5.15.1 What is already there

- **One binary, one dispatch.** `main` (`holon/src/main.rs` 648) matches its first word against
  holon's commands; nothing looks at the file it was started from.
- **A host is assembled from parts.** `serve` (`main.rs` 704–1202) opens the store, the policy,
  the secrets, the folders and databases, loads the deployments into one map, then starts the
  clock (`spawn_clock_scheduler`, 1111), the file watcher (`arrivals::spawn_watcher`, 1112), the
  thread that syncs the logs every second (1120–1146), the witness (`heads::start`, 1002), the
  listeners (`sync_listeners`, 1182) and the control plane (rouille, 1014).
- **What runs a flow takes the map, not the server.** `run_action` (`deploy.rs` 940),
  `serve_data` (`listeners.rs` 15), `fire_one_clock_tick` and `spawn_clock_scheduler`
  (`scheduler.rs` 101, 211), `spawn_watcher` (`arrivals.rs` 419), `expire_waiting`
  (`durable.rs` 727), `log_run` (`firing.rs` 894) and `sync_logs` (`recorder.rs` 704) each take
  the store, the deployments map and the deployments file's place. Only `start_listener`
  (`dataplane.rs` 117) takes the whole `AppState`, and reads those three and `data_limits` from it.
  So a host of one flow is the same functions over a map of one.
- **Deploying derives and checks.** `deploy` (`deploy.rs` 148): the unit's closure, `read_server`,
  `deployed_capabilities`, a `Manifest` of those and the requirements the unit itself declares
  (`Requirements::of_unit`, `permissions.rs` 323), the policy's refusals, what needs approval
  against the previous grant (`approval_needed`, 229), then `Grant::for_manifest(…, "deploy")`
  (252) and its `Permit` (269). A `Grant` (`permissions.rs` 803) holds the manifest's hash, the
  items, the requirements, when and how it was granted; its hash (991) covers the manifest, the
  items and lowered limits, and every log entry names it. `Grant::attention` (970) is the check,
  at every load, that a grant still covers what its unit derives.
- **A redeploy keeps what the flow had** (`deploy.rs` 295–366): its state, its log, its waiting
  runs, its channels, and a new `Version` with who deployed it.
- **Text to units.** `agent::plan` (`holon-author/src/agent.rs` 44) compiles a text into its root,
  every unit as a preimage (children first), its requirements, routes and `may`;
  `holon deploy` stages those units on a server (`deploycmd.rs` 150) and `holon check`
  (`holon-author/src/commands.rs` 152) prints what the flow may do.
- **Exports carry a closure.** An export (`verify.rs` 39, format `holon-runs/1`) holds the log's
  lines as written, its head, and every unit the runs ran as canonical bytes by hash;
  `verify::check` (338) runs each run again offline. Both are in the same binary as the server.
- **The record.** A deployment's log is `<deployments>.logs/<slug>.jsonl`, a hash chain
  (`recorder.rs` 1–24). Each handler has a level (C5): a `strict` run's line is synced before it
  answers, a `lazy` or `unrecorded` one returns at once (`firing.rs` 900) and the sync thread
  writes it within `SYNC_EVERY` (`recorder.rs` 671). Heads go to a witness after a sync, and wait
  in `pending.jsonl` across restarts (`heads.rs` 1–25).
- **Secrets** live in a vault per deployment, sealed under a key in the OS key store
  (`sealing.rs`), or as references the host resolves: `env:HOLON_SECRET_<NAME>`, a `--dotenv`
  file, `vault:` (`secret_sources.rs`). Every value is redacted from what is logged
  (`secrets.rs`), and `taint.rs` follows what is computed from one.
- **Folders and databases** are bound by an admin, kept in `folders.json` and `databases.json`;
  the server's own files can never be bound (`main.rs` 1081–1089).
- **The same user's commands find a running server** through a run file holding its address and a
  token made for that run, readable only by that user (`local.rs`, `publish` 104).
- **Releases** are built by cargo-dist for five targets (`dist-workspace.toml`: Apple silicon and
  Intel macOS, arm64 and x86-64 Linux, x86-64 Windows), as archives with checksums and GitHub
  build attestations, the editor built in (`embed-editor`, `holon/Cargo.toml` 100–109). Nothing
  is Authenticode-signed or notarized. An installed release (Windows, 2026-11-05) is 56.7 MB; a
  local `cargo build --release` without the editor is 40.2 MB.
- **§5.14 is designed, not built:** there is no `holon do` yet. `holon action` (`opscmd.rs` 333)
  posts an action to a server and prints its answer as JSON (`action_lines`, 368).

#### 5.15.2 The file

**The payload** is one record, `holon-app/1`, in the canonical encoding Holon hashes values with
(so it has one byte form, and its hash is `hash_value`'s):

| Field | What |
|---|---|
| `name` | the flow's name, its slug in the app (`Plan.name`) |
| `text`, `file` | the `.holon.ts` text as given, and its file name |
| `root` | the root unit's hash |
| `units` | every unit the root's closure holds, hash → canonical preimage (`Plan.units`) |
| `requirements` | what the unit asks beyond its graph (`Requirements::of_unit`) |
| `grant` | the builder's approval: the manifest's hash, its items, the approver, when (§5.15.4) |
| `witness` | a witness URL the builder chose, or none (§5.15.5) |
| `runtime` | the holon version, target and flavor (`slim` or `full`) it was built for |

No field holds a secret, a binding or a path: a payload can't carry one (§5.15.6).

**The frame.** The payload is followed by a 64-byte trailer: `HOLONAPP`, a frame version, the
payload's length, its BLAKE3 hash, and `HOLONAPP` reversed. The runtime finds the frame, checks
the hash, then reads the record.

**Where the frame sits depends on the format**, because appending to a signed executable breaks
its signature (§5.15.8):

- **ELF (Linux):** appended to the runtime. The loader ignores bytes past the segments it maps.
- **PE (Windows):** a resource, `RT_RCDATA` named `HOLONAPP`, so it is inside the image a
  signature covers.
- **Mach-O (macOS):** a section `__HOLON,__app` in a segment of its own, then the file is
  ad-hoc signed again, as Apple silicon runs nothing unsigned.

The frame is the same bytes in all three, so one reader finds it in any of them on any OS: the
runtime reading itself, and `holon inspect` reading a file it doesn't run (§5.15.7).

**At start.** Before `main`'s dispatch, the runtime reads its own file (`current_exe`) for a
frame. None: it is holon, as today. One: it checks, in order, the frame's hash; every unit
against its hash; that the root resolves and is a server (`read_server`); that the manifest the
units derive is covered by the payload's grant; and that its own version and target are the
payload's `runtime`. Any failure exits 70 with what failed. **A damaged app never falls back to
being holon**: a file named `todo` that quietly became a server, an MCP host or an updater would
be worse than one that refuses.

An app's words are its flow's. Holon's commands (`serve`, `deploy`, `mcp`, `update`…) aren't
there; `--holon` is the only word an app reserves (§5.15.7), with `--help`.

#### 5.15.3 What it runs as

What the file does follows the flow's handlers (`read_server`'s `Server`):

| The flow has | `app` alone | `app <action> …` | `app serve` |
|---|---|---|---|
| actions only | lists its actions (`--help`) | runs that action once, and exits | refused: nothing to serve |
| routes, clocks or file handlers, no actions | serves | — | serves |
| both | lists its actions | runs that action once (or in the serving app, below) | serves |

`serve` and `help` are refused as action names at build in a flow that has anything to serve.

**Actions are a command line,** by §5.14's mapping, the same code as `holon do`:
`todo add "buy milk" --due fri` is `{args: ["buy milk"], due: "fri"}`, a bare `--force` is
`{force: true}`, `-` reads stdin into `stdin`, `@notes.txt` sends that file's text as the field,
`-o out.csv` writes the answer to a file. A text answer prints as it is, anything else as JSON
(`--json` for the whole envelope). The exit code is 0 below 400 and 1 at or above it, unless the
answer is an envelope with a whole-number `exit`. `--help` lists the actions and the input
fields each reads. The run is `run_action`'s (`deploy.rs` 940) on the map of one, with `by` the
OS user (`local::user`, `local.rs` 86), so it is recorded as a server's action is.

**Routes, clocks and file handlers are a daemon.** `app serve` starts what `holon serve` starts
for one deployment and nothing else: its listener on the flow's port (bound to loopback unless
`--host IP` opens it, with `plain_http_warning`'s warning), the clock (which also expires waiting
runs, `scheduler.rs` 215), the file watcher, the sync thread and the witness. It runs in the
foreground until stopped (Ctrl-C, SIGTERM, a service manager's stop), then syncs its logs and
exits. Making it a service (systemd, launchd, a Windows service) is the runner's, with the
service manager they use.

**One process writes a data folder.** The app holds an exclusive lock on `<data>/lock`
(`File::try_lock`, std since Rust 1.89) while it runs. While `app serve` holds it, `app add …`
doesn't run the action itself: it finds the serving process through a run file in the data
folder, an address on loopback and a token made for that run, readable only by the user, as
`local.rs` publishes a server's (104), and sends the action there. So an action from the command
line and a request on the port are runs of one host, ordered by its claims. With no serving
process, `app add …` takes the lock, runs, syncs and exits; a second one waits for the lock
(up to `--wait`, 10 s by default).

**Runs that wait.** An action that parks at an `await` is kept as on a server (its `ParkedRun` in
the deployments file); `app add …` says it is waiting, prints where it waits, and exits 0. It
continues when its resume route is called while the app serves, or through an action that
resumes (§5.14, slice 2). A one-shot start calls `expire_waiting` before its run, so a deadline
that passed while nothing ran is delivered then.

**One flow, one app.** A flow that runs another flow's action (`RunsAction("billing/charge")`) is
refused at build: the app has no other flow to run it (§8, fork 36).

#### 5.15.4 Who approves, and when

**Both: the builder at build time, and the person who runs it on its first run, the build's grant
a ceiling the first run can narrow and never widen.**

1. **At build, the builder approves.** `holon build` derives what `deploy` derives, by the same
   calls (`closure`, `read_server`, `deployed_capabilities`, `Manifest::new` over
   `Requirements::of_unit`), refuses what the default policy never grants, prints `check`'s
   words, and asks to approve every item (`--approve` answers yes, as `holon deploy --approve`
   does, `deploycmd.rs` 43). The approval goes into the payload: the manifest's hash, the items,
   the approver and when. The approver is `--approver "Ada Lovelace <ada@example.com>"`, or the
   OS user. That is a name the builder states, not a proof; the proof of who made the file is its
   signature (§5.15.8). There is no build without an approval, and the runtime refuses a payload
   without one.
2. **On first run, the runner accepts.** Before its first run on a data folder, the app shows who
   built and approved it, what it may do (the same words), any witness, its data folder and where
   its unbound folders will be, and asks. A yes writes the grant to the data folder's deployments
   file as its deployment's `Grant`, `by: "accepted"`, and the runner as the version's `by`. A
   script or a service manager accepts ahead with `--accept 3f9a1c` (a prefix of the grant's
   hash, so it accepts that grant and no other) or `HOLON_APP_ACCEPT`; with neither and no
   terminal, the app exits 77: "not accepted: run it once by hand, or pass --accept 3f9a1c". The
   runner may narrow it later (`app --holon revoke ITEM`, an optional item, or `--holon lower`, a
   limit), through `Grant::change` (`permissions.rs` 888), which records each change. Never wider:
   `Grant::change` grants only what the manifest asks for, and the app checks the result against
   the payload's items.
3. **Every start checks the ceiling.** The app derives the manifest from its units again (never
   from the payload's list), and refuses to run when the payload's grant doesn't cover it (the
   payload was changed), when the kept grant holds an item the payload's doesn't (the data folder
   was changed), or when `Grant::attention` says the kept grant doesn't cover the manifest under
   the runner's policy. Then `Permit::new`, as a deploy builds one.
4. **A new build on the same data folder is a redeploy.** Its payload's grant is the new ceiling.
   The runner is asked only for what the kept grant didn't already cover (`approval_needed`
   against the previous grant, `deploy.rs` 229); items the new build no longer asks for are dropped
   from the kept grant.

**The runner's policy.** A `holon.toml` in the data folder, if there is one, is the policy, as a
server's is: its ceilings and refusals hold. Without one, the defaults. A ceiling lower than what
the build approved stops the app as it pauses a deployment.

**Why both.** The build's approval is the promise as it stands: a person reads what the flow may
do before it runs, once, someone the team can name, and it travels with every copy unchanged. The
runner's is there because the runs happen on their machine, with their data, their folders and
their network: they can see the same list before anything runs, and they can narrow it. One
without the other fails someone: a build-only approval runs on a stranger's machine unread by
them; a first-run-only approval lets a file change what it asks for between copies.

**Unrecorded handlers** (`Unrecorded("…")`) are items like any other, so both people approve
them.

#### 5.15.5 Records

**The data folder** is `--data DIR`, else `HOLON_APP_DATA`, else `~/.holon/apps/<name>/` (beside
`local::holon_home`'s other folders), made owner-only as `serve` makes its own
(`create_private_dir`, `main.rs` 987). It is named by the flow's name, not its hash, so the next
build of the flow finds its state. Its layout is `holon serve --data`'s, with one deployment in
it: the store, `deployments.json`, `deployments.logs/<name>.jsonl`, its events, `secrets.json`,
`folders.json`, `databases.json`, and `holon.toml` if the runner writes one. It records the flow's
name and refuses another's; a second copy of an app takes a second `--data`.

**The store is seeded from the payload** at every start: each unit put by its hash, so a unit
already there is the same bytes. Exports then find every unit a run ran, old builds' included.

**Levels are the request path's.** Each handler's level comes from its unit (C5), and `log_run`
writes as it does on a server: a `strict` action's line is synced before its answer is printed;
a `lazy` one's is synced as the one-shot app exits (one `sync_logs`), so a clean exit loses
nothing and a crash loses what a crash loses on a server; an `unrecorded` one is counted in the
deployment's tally. A one-shot run costs a process start, the units' checks and at most one sync.

**Verify.** `app --holon export [-o FILE]` writes a `holon-runs/1` export of its log
(`verify::export_of`, `verify.rs` 117), and `holon verify FILE` checks it on any machine, as for
a server's. `app --holon verify` runs the same check (`verify::check`, 338) on its own log in
place.

**A witness.** Two ways, the same mechanism (`heads.rs`):

- **The builder's,** `holon build --witness URL`: in the payload, shown in both approvals, and
  the runner can't remove it (fork 35). It is for a team that wants every copy's runs witnessed.
- **The runner's,** `--export-heads URL` or `HOLON_EXPORT_HEADS`, as `serve` takes it.

The token is `HOLON_EXPORT_HEADS_TOKEN` only, never in the file. A one-shot app sends the heads
queued after its exit's sync, waiting up to 2 s; what isn't taken waits in `pending.jsonl` for
the next start, as a server's do across a restart.

#### 5.15.6 Secrets and bindings

**Secrets never go inside the binary.** The payload has no field for one, `holon build` reads no
secret store and no environment, and the text names secrets, never holds them. At run time a
secret the grant names (`SecretRead("name")`) comes from, first found first:

1. `HOLON_SECRET_<NAME>` in the environment, or a line of `--dotenv FILE`
   (`secret_sources.rs`'s `env:` reference);
2. a reference the runner set: `app --holon secret set NAME --from vault:kv/app#token`;
3. a value sealed in the data folder's `secrets.json` under the OS key store (`sealing.rs`;
   `--secrets-key` as `serve` takes it), set by `app --holon secret set NAME` or by the prompt.

**The prompt.** On a terminal, a run whose grant names a secret with no value asks for it before
the run starts, without echo, saying where it is sent ("slack_webhook_path, sent to
https://hooks.slack.com"), and seals it. With no terminal, the app exits 78, naming the secret
and its variable. Redaction (`secrets.rs`) and taint (`taint.rs`) are unchanged, so the app's log
never holds the value either.

**Folders and databases are bound at run time,** because a flow names things and the host
decides where they are, and here the host is the runner's machine:

- `--folder notes=./data` and `--database ledger=…` (what `holon database add` takes). A path is
  read against the folder the command runs in, then kept whole (`paths::normalize`), in the data
  folder's `folders.json` or `databases.json`, so the next run needs no flag. A flag naming
  another place later replaces it, and says so.
- **An unbound folder** is `<data>/folders/<name>/`, made when first needed and shown at first
  run. Never the folder the command runs in: a flow doesn't reach where the person stands except
  through `@file` and `-o`, which the client side reads and writes (§5.14).
- **An unbound database** has no default: a run that needs it fails, naming the flag.
- **What can't be bound** is the server's rule (`main.rs` 1081–1089): the data folder, its key
  file and `~/.holon`.

#### 5.15.7 What the binary says of itself, and why to ask holon instead

`app --holon` prints, from its payload and its data folder:

```
todo-cli, built with holon 0.1.0-rc.34 (slim, x86_64-unknown-linux-gnu)
unit 4be1…: 23 units, each matching its hash
approved by Ada Lovelace <ada@example.com> on 2026-11-07, grant 3f9a1c…
accepted by ada on 2026-11-07
it may:
  read the clock
  read items of its collection "todos"
  put and delete items of its collection "todos"
its runs and data: /home/ada/.holon/apps/todo-cli (12 runs, the chain unbroken, head 9b2e…)
```

`app --holon text` prints the text it was built from; `app --holon extract DIR` writes it and the
units (as an export's `units` map), so `holon check DIR/todo-cli.holon.ts` shows the same unit
under the same holon version, and anyone can read and check the flow without running it.

**A binary's word about itself is only as good as the binary**: a changed runtime can print any
of that. So the check to trust is the person's own holon reading the file without running it:

```
$ holon inspect ./todo
todo: holon 0.1.0-rc.34 for x86_64-unknown-linux-gnu, slim, and the flow todo-cli
  the frame: whole (BLAKE3 c41d…)
  units: 23, each matching its hash; its text compiles to the same unit
  approved by Ada Lovelace <ada@example.com> on 2026-11-07, grant 3f9a1c…
  it may: …
  the rest of the file: holon's released runtime, byte for byte
```

The last line comes from rebuilding: the released runtime for that version and target, with this
payload placed in it, compared with the file (§5.15.9). A file that matches runs holon's code and
this flow, and nothing else. "Compiles to the same unit" needs the same holon version as the
build; another version says it can't tell.

#### 5.15.8 Platforms

**Targets.** `holon build --target TRIPLE`, any of the five a release builds. The runtime for the
builder's own target is its own file (`current_exe`, the full flavor) or the slim asset; another
target's comes from the GitHub release of the builder's version, `holon-runtime-<target>`,
downloaded over HTTPS (`ureq`, already a dependency), checked against the release's
checksum, and cached in `~/.holon/runtimes/<version>/<target>/`. `--runtime FILE` takes one
already downloaded, for a builder offline. A build that isn't a release (a developer's
`cargo build`) has no assets of its version, so it builds for its own target only, and says so.

**A slim runtime.** An app needs the core, the author crate (for `--holon`'s words and the first
run's), the host, the module engines, the data plane, folders and SQLite. It doesn't need the
editor (`embed-editor`), the terminal editor (ratatui), the LSP and MCP servers, the control
plane (rouille), accounts and sign-in (argon2, OIDC), the updater (axoupdater, self-replace) or
the folder picker (rfd). A feature, `app-runtime`, builds holon without them, and each release
publishes it per target beside the archives. The editor is most of the gap between an installed
release (56.7 MB) and a build without it (40.2 MB); how small the rest gets is measured in the
slice that builds it. The Postgres, MySQL and MongoDB engines stay in the full flavor only;
`--runtime full` builds on it (fork 34).

**Signatures.** Appending to a signed executable breaks the signature, so where a platform checks
one the payload goes inside the image and the file is signed after:

- **Linux** checks no signature. The payload is appended; provenance is the release's
  attestation of the runtime plus `holon inspect`'s comparison.
- **Windows:** an appended payload would sit after a signature's certificate table, which
  `WinVerifyTrust` rejects, and scanners treat appended data as suspect. As a resource it is part
  of the image `signtool sign` covers, so the builder signs the app after building, with their
  own certificate. The release runtimes aren't Authenticode-signed today; if they become so,
  inserting the resource invalidates that signature, and the builder's replaces it.
- **macOS:** the linker ad-hoc signs arm64 binaries, Apple silicon kills code whose signature
  doesn't verify, and appended bytes make it not verify. So the payload is a section, and the
  build signs the file ad hoc again, which is enough to run it on the machine that built it. To
  hand it to others the builder signs it with a Developer ID (`codesign --options runtime`) and
  notarizes it (`notarytool`); unsigned, Gatekeeper quarantines a downloaded app.

`holon build` doesn't sign with a builder's key: keys and tools are the platform's, and the
manual's guide gives the commands. **The engine** that places the payload and re-signs ad hoc in
pure Rust is `libsui`, the one `deno compile` uses (MIT; ELF, PE resources and Mach-O sections),
or holon's own code for the three if `libsui` doesn't hold up in slice 1 (fork 32).

#### 5.15.9 Updates and reproducibility

**The same inputs give the same bytes.** A build is a function of the runtime's bytes and the
payload: the name, text, units, requirements, grant (items, approver, when) and witness. The
payload's encoding is canonical; the grant's time is `--at`, else `SOURCE_DATE_EPOCH`, else now,
so a pipeline that sets it builds byte for byte again; placing a resource or a section writes no
time and orders what it writes; an ad-hoc signature is a hash of the pages. A test builds twice
and compares, and CI builds the same app for Linux on Linux and on Windows and compares.

**Checking a file by rebuilding.** `holon inspect` reads the payload, takes the released runtime
it names, places the payload, and compares with the file (a signature stripped first: PE's
certificate table, Mach-O's code signature). Equal means the file is that release's runtime and
this payload. This is how anyone checks an app from someone else without trusting its own words
(§5.15.7).

**Updates.** A new version of an app is a new file. Run on the same data folder, it is a redeploy
(§5.15.4, point 4): state kept, the log going on, a version recorded, the runner asked only for
what is new. A new holon release changes nothing in apps already built; rebuilding with it gives
a new runtime and, where the catalog changed, a new root, as redeploying under a new server does.
**An app never updates itself** (§5.15.12). The data folder records the runtime version that last
wrote it; an older runtime than that refuses it, naming the version it needs, rather than read a
format it may not know.

#### 5.15.10 The checklist (§4)

1. **The item and its words:** none new. Every item is the flow's own, approved as a deploy's
   are; the build's witness is shown in both approvals as "sends each run's head (a hash, no run
   data) to https://…".
2. **The resource:** folders and databases bound by the runner at run time (§5.15.6), the port
   the flow's. Credentials from a secret source, never the file.
3. **Reads are the run's input:** unchanged; the same recorder.
4. **Effects land when the run finishes, at most once:** unchanged; a one-shot app exits only
   after its run's effects and record have landed.
5. **Sharing:** one process per data folder (the lock); claims within it. Two apps bound to one
   folder share it as two servers would: locks hold within a process only (§6's words).
6. **Triggers:** unchanged, but clocks and file handlers run only while `app serve` runs. A file
   that arrived meanwhile starts its run at the next serve (the watcher keeps what it has seen,
   `arrivals.rs`); a clock fires once as serving starts, as a server's do at load (`arm_clocks`,
   `main.rs` 408), and doesn't catch up on the ticks it missed.
7. **Secrets:** unchanged taint and redaction; values from the environment, a vault or the
   prompt, sealed in the data folder, never in the file (§5.15.6).
8. **Limits:** the build's grant holds the limits asked for; the runner's `holon.toml` can lower
   ceilings; nothing raises one past the build's approval.
9. **Verify:** `app --holon export` and `holon verify`, as a server's; `holon inspect` checks the
   file itself (§5.15.7, §5.15.9).
10. **Seeing it:** `holon build`'s and the first run's approval, `app --holon`, `holon inspect`,
    the manual's guide to building apps (signing included) and its permissions page.
11. **The sample:** `todo-cli` (§5.15.11). No outside service, so no stand-in.
12. **The engine:** `libsui` or holon's own placement code (fork 32); `ureq` for runtimes,
    already a dependency; the slim runtime is a feature of the holon crate.

#### 5.15.11 Samples and slices

**`todo-cli`**, the sample §5.14 named, in the language, as `fmt` writes it:

```ts
// Todo list: a flow as a command-line app.
//
// Its actions are its commands. Try it: holon build todo-cli.holon.ts -o todo --approve, then
//   ./todo add "buy milk" --due fri     "added buy milk"
//   ./todo list                          the todos, by title
//   ./todo done "buy milk"               "done buy milk"
// Deployed on a server, holon do todo-cli add "buy milk" does the same.
import { action, flow } from "holon"
import todos from "collection:todos"
import { now } from "holon:clock"

flow("todo-cli")

// Adds a todo under its title, with when it is due.
export const add = action((input) => {
  const title = input.args[0]
  todos[title] = { due: input.due ?? "", since: now }
  return `added ${title}`
})

// The todos, by title. It only reads, so it is lazy.
export const list = action(() => todos.list({ limit: 100 }), { log: "lazy" })

// Marks one done by removing it.
export const done = action((input) => {
  delete todos[input.args[0]]
  return `done ${input.args[0]}`
})
```

It is an ordinary flow too: `editor/src/samples.test.ts` builds it, the e2e tests deploy it, and
on a server `holon do` drives it. A Rust test builds it with the holon under test
(`holon build … --approve --at 0`), runs `todo add`, `list` and `done` with `--accept`, checks
their output and exit codes, then `todo --holon export` and `holon verify` on the export, and
`holon inspect` on the file. It is run live once by hand before its commit.

**Slices:**

1. **One target, actions:** the payload, the frame and its placement in all three formats; the
   start's checks; one-shot actions in-process with §5.14's mapping (built here if `holon do`
   isn't yet, shared with it); approval at build and on first run; the data folder and the
   lock; records, `--holon`, `--holon export` and `verify`; secrets from the environment, the
   prompt and the sealed file; folder and database bindings; `holon inspect` without the
   runtime comparison; `todo-cli` and its test.
2. **Serving:** `app serve` (listeners, clocks, file handlers, expiry), actions sent to the
   serving process, witnesses (the builder's and the runner's), stopping cleanly. A test builds
   `notes-api` and `heartbeat` and drives them.
3. **Every target:** the slim runtime and its release assets (a publish job beside
   `legacy-installers`), downloading and caching runtimes, `--runtime FILE`, the reproducibility
   tests, `holon inspect`'s rebuild and comparison, the ad-hoc re-signature checked on CI's macOS
   runner, the manual's signing guide.
4. **Later:** a question mid-run (§5.14, slice 2), progress, a `--holon service` that prints a
   unit file for systemd or launchd.

Not scheduled in §7: Tier 3, a step when it is asked for.

#### 5.15.12 Refused, with the reason

| Refused | Why |
|---|---|
| A secret inside the binary, encrypted or not | Anyone with the file has it, and a file is copied. Values come from the runner's environment, vault or prompt. |
| A build or a run without approval (`--no-approve`, a payload without a grant) | The promise is that a person read what a flow may do before it runs. Both approvals are cheap: one answer at build, one at first run. |
| A switch to run without recording (`--no-log`) | Every run is recorded at its handler's level; `unrecorded` is a handler's level a person approves, not a switch the runner flips. |
| An app that updates itself | What runs must be the bytes that were inspected and accepted. A new version is a new file, and its new items are asked for. |
| An app falling back to holon's commands when its payload is damaged | A file named `todo` must not become a server, an MCP host or an updater. It exits 70. |
| The folder the command runs in as a default binding | A flow names things and the host decides where they are; a command's working folder is wherever the person happened to be. `@file` and `-o` reach it, by the person's hand. |
| A payload that is obfuscated or compressed past reading | It is meant to be extracted and checked with `holon check` and `holon inspect`. |
| Several flows in one app, or an app calling another flow | One flow, one data folder, one grant. Fork 32. |
| Cross-building from a build that isn't a release | No released runtime of its version exists for another target to compare with. |

#### 5.15.13 Forks

§8, forks 32 to 39, each with its lean: the engine (32), overriding the port (33), which runtime
by default (34), a builder's witness the runner can't remove (35), several flows in one app (36),
two apps under one name (37), the approver's identity (38), exit codes (39).

#### 5.15.14 What stays true

- **Approval before a run:** a person approved what the flow may do at build, and the person
  running it accepted it on their machine; neither can be widened by the other or by the file.
- **The record:** every run is recorded at its handler's level in a hash-chained log, exported as
  a server's is, and checked offline by `holon verify`.
- **Secrets:** never in the file, never in the log; taint as on a server.
- **A flow names things; the host decides where they are:** the runner binds folders and
  databases; the flow never sees the folder the command runs in.
- **No code outside a boundary the manifest describes:** an app is holon's released runtime and
  one flow, and `holon inspect` shows it byte for byte.
- **Content addressing:** the units in the file are checked against their hashes at every start;
  the same inputs build the same bytes.

## 6. One pattern for remote files: folder backends

SFTP and object storage are not new kinds of thing for a flow. They are **folders somewhere
else**. So a flow keeps `folder bank`, `bank[name]`, `on file bank (f)` and `FileRead`,
`FileWrite`, `FileArrival`; the admin binds the name to `sftp://sftp.bank.example/outgoing` or
`s3://payments-archive/2026/` instead of a path. The language doesn't grow; a backend is host
work only.

What a backend must say:

- **Where it is, in the grant.** Consent should know that a file leaves the machine: the deploy
  approval shows the binding (`read files in folder "bank": sftp.bank.example`).
- **Trust.** SFTP pins the server's host key in the binding (an unknown key refuses the
  connection); S3 signs every request (SigV4) with credentials from a secret source.
- **Arrival.** No change notices: the host lists the folder on an interval, and a file has
  settled when its size and modified time are unchanged across two listings. The interval is a
  limit.
- **Writes.** Written under a temporary name and renamed into place (SFTP), or one PUT (S3), when
  the run finishes. A rename some servers refuse is the L4's to settle.
- **Sharing.** Commit locks and claims hold for this server only: a remote folder has no lock
  another server would see. The L4 says so where it is bound.
- **Reads** are logged as a local folder's are: by hash, texts in `blobs.rs`.

The same pattern takes SMB shares and Azure Blob later, each a backend.

## 7. The implementation plan

Decided 2026-10-29: host capabilities are built in three tiers, in order, ahead of a job asking
for each. **Tier 1** is the essential host capabilities a flow has no way to reach today.
**Tier 2** is what an operator needs to run Holon in production and answer for it. **Tier 3** is
the rest, taken when someone asks. Within a tier, steps with no dependency between them run side
by side; a tier starts when the one before it has landed.

**How every step runs:**

- **Its design first,** in its section here: §4's checklist answered in numbered subsections, as
  §5.11 and §5.12 are, reviewed before code. Then slices, each one commit.
- **Each slice ships whole** (CLAUDE.md): code and tests, a sample flow in
  `crates/holon-author/samples/` run live once, the manual's guide and permissions page, and an
  e2e test that deploys the sample against a local stand-in, so CI needs no outside account.
- **Catalog changes** run `wasm-pack test --node crates/holon-wasm` and bless the manual's page
  of leaves (`HOLON_BLESS=1`).
- **One worktree per step,** so steps that run side by side never share a checkout.
- **A release candidate when a step lands,** its CHANGELOG entry saying what, if anything, to
  redeploy.

Sizes are relative: **S** a slice or two, **M** a few slices, **L** several slices and a new
engine.

### Tier 1: essential host capabilities

**Order, revised 2026-11-09:** after 1.3, step 1.6, then 1.8 (its rest segment first, 1.8a, then
answering with a file: §5.11.8), then 1.9. Steps 1.4, 1.5 and 1.7 are deferred, taken when asked;
1.8 was deferred with them from the 2026-11-08 revision until this one. 1.6 needs 1.4's request
signing only for SQS (fork 8): its design settles the queue engine with 1.4 deferred.

| Step | What | Design | Needs | Size |
|---|---|---|---|---|
| 1.1 | **Value kinds:** `Bytes`, built (§5.10.2); `Decimal` decided as text leaves, built in 1.2 (§5.10.1) | §5.10 (here) | — | M |
| 1.2 | **Formats:** exact decimals, CSV, XML, dates (UTC and offsets), zip and gzip, JWT verify, PGP encrypt and verify with a public key, in six slices (§5.1.1); 1.2a, decimals and CSV, built (§5.1.2); 1.2b, offsets and business days, built (§5.1.4); 1.2c, XML, built (§5.1.6); 1.2d, zip and gzip and bytes written to a file, built (§5.1.8); 1.2e, JWT verify, built (§5.1.10); 1.2f, PGP encrypt and verify, built (§5.1.12) | §5.1 (here) | 1.1 | M |
| 1.3 | **Email:** send, recipients in the grant; receive as `on mail`, with its sender's verdict and attachments, in three slices (§5.2); 1.3a, sending through SMTP, built (§5.2.3); 1.3b, receiving through IMAP, built (§5.2.5); 1.3c, mailboxes that log in with OAuth, built (§5.2.7) | §5.2 (here) | 1.1 for attachments | L |
| 1.4 | *Deferred.* **Remote folders:** the backend seam in `folders.rs`, then SFTP and S3-compatible storage (§6) | §6 (here) | 1.1 for binary files | L |
| 1.5 | *Deferred.* **Secure connectivity:** keys the host holds (PGP decrypt and sign) and client identities (mutual TLS, a private CA) (§5.3, §5.4) | §5.3, §5.4 (here) | 1.1, 1.2 | M |
| 1.6 | **Messaging:** one subscription runtime (position, once per message, backpressure, reconnect), feeds (SSE, WebSocket, Postgres `LISTEN`) and one queue engine, publish and consume (§5.5, §5.9); designed in slices (§5.9.1), all built: 1.6a the subscription runtime and server-sent events (§5.9.3), 1.6b WebSocket feeds (§5.9.5), 1.6c a feed resumed (§5.9.7), 1.6d a feed's gaps (§5.9.8), 1.6e the queue (§5.9.10, live against ElasticMQ and moto; real AWS still to come), 1.6f a Postgres channel (§5.9.12) | §5.5, §5.9 (here) | 1.4's request signing, for SQS (fork 8) | L |
| 1.7 | *Deferred.* **Commands:** programs an admin binds, typed arguments, logged output, killed on timeout (§5.7) | §5.7 (here) | — | M |
| 1.8 | **Static files:** a route's rest segment (`/*path`, routing, no item), and answering with a file from a folder (`site.serve(name)`, `FileServe`), streamed by the host and checked against the handle's hash; in slices (§5.11.8), 1.8a the rest segment (built, §5.11.13), 1.8b answering with a file (built, §5.11.15) | §5.11 (here) | — | M |
| 1.9 | **Inbound WebSockets:** `websocket PATH { open … message … close … }`, a run per message, replies and channel packs out, a record per connection (§5.9) | §5.12 (here) | 1.1 for binary frames | M |

- **Samples, one at least per step:**
  - 1.1 `file-checksums` (a binary file's hash, into a manifest);
  - 1.2 `payment-batch` (a CSV of payouts in, a pain.001 out, its control sum exact);
  - 1.3 `approve-by-email` (a payout over a limit mails an approver its resume link) and
    `invoice-inbox` (an invoice's attachment into a folder);
  - 1.4 `bank-drop` (statements arriving over SFTP, into a collection);
  - 1.5 `bank-statements` (SFTP, PGP decrypt, camt.053 parsed);
  - 1.6 `price-feed` and `payout-events`, and `trading-bot` (a feed of prices, conflated; a
    lazy handler for the prices and a strict one for the order; a client order id from the
    signal; an order over a limit waiting for approval; reconciliation on start), against a
    stand-in exchange;
  - 1.7 `nightly-reconcile` (a bound program on a clock, its output into a folder);
  - 1.8 `site` (a built web app and its API, with a deep link falling back to `index.html`);
  - 1.9 `room` (a chat: joins, messages and leaves over one connection per browser).
- **Stand-ins for e2e:** an SMTP sink and an IMAP server, an SFTP server and an S3 stub, an HTTPS
  server that requires a client certificate, an SSE and WebSocket stub, a test program; each
  started by the e2e setup. 1.8 and 1.9 need none: a fixture folder, and the browser as the
  client. A queue engine that can't be stubbed in-process (a real broker) is
  settled in step 1.6's design (§5.5, §5.9).
- **Alongside 1.7:** retire `egress` and `Spawn` from `Capability` and the catalog (§8, fork 7),
  `CommandRun` taking `Spawn`'s place. S.

**Tracks.** 1.1 is the one most others stand on, so it starts first. Then:

```
A  1.1 ──► 1.2 ──► 1.5 (deferred)
B  1.4  (deferred; text files at once; binary files once 1.1 lands)
C  1.3  (text mail at once; attachments once 1.1 lands)
D  1.6, 1.9  (one engine, tokio-tungstenite; 1.9's text frames at once, binary once 1.1 lands)
E  1.7  (deferred)
F  1.8  (the rest segment, then FileServe)
```

Six tracks, so six worktrees at most; tracks that touch the same files (`kind.rs`, the catalog,
`tests.rs`) merge into `main` one at a time, each rebased on the last. Since the 2026-11-09
revision, C, D and F are the ones running, one after another: 1.3 to its end, then 1.6, then 1.8,
then 1.9.

### Between Tier 1 and Tier 2: the TypeScript slices

Decided 2026-11-08: once Tier 1's steps 1.1 to 1.3, 1.6, 1.8 and 1.9 have landed (the rest
deferred, above; 1.8 added 2026-11-09), the TypeScript syntax's slices 6 to 11 come next
([typescript-syntax](typescript-syntax-L4.md) §15): computing loops, calls and requests that
wait, waits a route's caller outlasts, waiting loops, failures with a reason, and editors and
tools. Tier 2 starts when they have landed; Tier 3 and the deferred Tier 1 steps are taken when
asked. Two of Tier 2's steps stand on deferred ones: 2.3 needs 1.4, and 2.4 needs 1.7.

### Tier 2: operationally essential

| Step | What | Design | Needs | Size |
|---|---|---|---|---|
| 2.1 | **Approvals by role:** `await approval(role: …)`, the editor's inbox, the approver in the log, four eyes, *n* of *m* (§5.6) | §5.6 (here) | — | M |
| 2.2 | **Telemetry and audit export:** OpenTelemetry over HTTP (a trace per run; metrics for runs, failures, latency, a feed's or queue's lag), the activity log and run heads to a SIEM, health and readiness endpoints (§2's server features) | a new section of §5 (here) | 1.6 for lag | M |
| 2.3 | **Who touched what:** resources with ids, addresses and a timeline across deployments ([host resources](host-resources-L4.md), designed), and each connection's own record (§8, fork 20) | host-resources-L4, revised there | 1.4, 1.6 | L |
| 2.4 | **A sandbox for agents:** walls from the grant, the operator's container runtime first, then Linux's own; the outbound proxy; writes at commit; agents' shell through MCP (§5.8) | §5.8 (here) | 1.7 | L |
| 2.5 | **History:** one timeline per deployment of its runs, deploys and grant changes, and restoring its code, its state or both to any earlier point, as a new entry ([history](history-L4.md), designed) | history-L4 | 2.1 for approved restores | M |

Samples: 2.1 `four-eyes-payout`; 2.4 `agent-reconcile` (an agent's script in the sandbox,
reading statements, writing a report); 2.5 `price-rollback`. 2.2 and 2.3 are the server's, shown in the server manual
and checked by e2e tests of their own.

If a first outside job comes before Tier 2, **2.1 is the step to pull forward**: it needs nothing
from Tier 1, and it is the capability closest to the pitch.

### Tier 3: nice to have

Taken when a job or a user asks, each with its design here and its sample all the same:

- **Persistent terminals** (a pseudo-terminal, SSH) and WebSocket sessions (§5.7, §5.9).
- **Sockets for payment networks:** ISO 8583, FIX, SMPP (§5.9). Promoted to Tier 1 if the first
  job runs on a card processor's link.
- **More engines:** the queues 1.6 didn't take (Kafka, NATS, SQS or RabbitMQ), MQTT, gRPC streams.
- **Mailboxes through OAuth:** Microsoft Graph, the Gmail API (§8, fork 2).
- **Stronger or wider sandboxes:** a micro-VM, TLS inspection at the proxy, a WASI backend
  (§8, forks 15 and 16).
- **A flow as one executable,** `holon build` (§5.15, designed), after §5.14's `holon do`, whose
  argument mapping it shares.
- **The rest:** time zones (fork 3), PDF output, SMB and Azure Blob folder backends, keys in a
  KMS or an HSM, a model resource with a budget.

## 8. Forks, surfaced

1. **Decimals as text leaves or a value kind.** A `Decimal` value, like `Bytes`, takes a new tag
   (no existing hash moves) but needs its JSON form, its place in the log, the bridge, the editor
   and every leaf that takes a number; leaves over text need none of that. *Lean:* decided in
   step 1.1's design (§5.10) with bytes, whose boundary answers make a `Decimal` value cheap;
   text leaves if that design finds a reason against. *Decided* (2026-11-08, §5.10.1): text
   leaves, in 1.2. The sources of exact decimals already give text (`NUMERIC`, CSV, XML, string
   amounts), and the syntax has no decimal for a value kind to be written with.
2. **Mailboxes that need OAuth.** Microsoft 365 has turned passwords off for IMAP and SMTP, and
   Google steers to OAuth. Either the host does OAuth (a refresh token as the secret, the host
   refreshing it), or those mailboxes are reached through their HTTP APIs (Microsoft Graph, Gmail
   API) as other backends.
   *Lean:* SMTP and IMAP with passwords first (most providers and relays take them), Graph as the
   second backend if the job is on Microsoft 365. *Decided* (2026-11-09, §5.2.6): the host does
   OAuth, logging in to the same IMAP and SMTP with `XOAUTH2`, which Microsoft 365 and Google
   Workspace both take; Graph and the Gmail API stay in Tier 3, for a tenant that turns IMAP off.
3. **The tz database.** Time zones need it in the binary (a few hundred KiB) and in the wasm
   build, and its updates are releases. *Lean:* UTC and fixed offsets first; zones when a job asks.
4. **Recipients' grain.** Exact addresses, domains with `*`, or addresses read from a collection
   (the grant then says "to addresses in collection contacts"). *Lean:* addresses and domains;
   from-a-collection only as its own item.
5. **Remote folders as backends or as their own kinds.** Backends keep the language small, but
   hide that a folder is remote from the flow's text. *Lean:* backends, with the binding shown in
   the grant (§6).
6. **PGP decrypt as a host key, or a leaf with the taint rule relaxed.** *Lean:* a host key
   (§5.3). Relaxing taint for one leaf breaks the rule that nothing computed from a secret
   leaves.
7. **`egress`, `SecretAccess` and `Spawn`.** Retire them from `Capability` and the catalog, or
   keep them unused. `egress`'s tests (`holon-host` and `holon`) would move to another sink.
   *Lean:* retire `egress` and `Spawn` once nothing uses them (`CommandRun` takes `Spawn`'s
   place, §5.7); `SecretAccess` is the coarse form `SecretRead` replaced.
8. **Which queue first** (§7, step 1.6). SQS's API is HTTPS, so its e2e stand-in is an
   in-process stub and its signing is S3's (step 1.4); RabbitMQ is what self-hosted shops run,
   but its stand-in is a real broker in CI. *Lean:* SQS-compatible first, RabbitMQ in Tier 3.
   *Decided* (2026-11-09, §5.9.1): SQS's API first, its signing written in the host.
9. **Which user a command or terminal runs as.** The server's own user can read the server's
   files, the secrets key among them. Another OS user (or, on Linux, a namespace) per binding
   confines it, but needs setup Holon can't do alone. *Lean:* an optional user in the binding,
   and the admin page saying plainly when a command runs as the server's own user.
10. **A terminal shared by several deployments, or one each.** *Lean:* bound like a folder (for
    the server or `--for <slug>`), held by one run at a time either way.
11. **How long a terminal's recording is kept.** It can hold screens with customer data.
    *Lean:* as long as the runs it belongs to (`--keep-runs`), plus a `keep` like a folder's
    (`hash`, or text for a while).
12. **What holds a sandbox's walls on Linux:** the server itself (namespaces, seccomp, Landlock,
    cgroups), bubblewrap as a program it runs, or only the operator's container runtime. The
    server itself needs no extra install but is the most code to get right; a runtime is what
    ops teams already trust. Ubuntu 23.10 and later restrict unprivileged user namespaces
    (AppArmor), so the server's own walls need a profile there, and CI's runner with them.
    *Lean:* the operator's container runtime first (it is also how Windows and macOS get one),
    the server's own walls second.
13. **Windows and macOS without a runtime.** *Lean:* no sandbox, said plainly when one is bound
    (§5.8). Not weaker walls under the same words.
14. **A secret given to a sandbox.** Its taint can't be followed inside, so all that comes out
    counts as computed from it: withheld from the log, never kept. That makes such a run hard to
    check. *Lean:* allowed, the item saying so ("given the secret bank_token: what it answers
    isn't logged"); a command (§5.7) is the better fit when a program needs a secret.
15. **HTTPS through the proxy:** connections only (host, times, sizes), or the host ending TLS
    with a certificate authority the sandbox trusts, so requests are logged whole like
    `http.post` rounds. The second is inspection of the agent's traffic, which is the point, but
    breaks certificate pinning and is a key the host must guard. *Lean:* connections only first;
    inspection as the admin's option per sandbox.
16. **WASI as a backend.** *Lean:* later, for scripts that must replay exactly.
17. **A feed whose runs fall behind:** stop reading, or skip. *Lean:* stop reading by default
    (nothing is lost, the sender waits); `on_full: skip` only where the binding says, with
    `e.missed`.
18. **Heartbeats while no run is there.** The host speaks a protocol on the deployment's behalf,
    outside any run. *Lean:* only from templates fixed in the binding, nothing a run computed,
    each logged in the connection's record.
19. **Private addresses for what an admin binds.** A socket or a feed on a private network is the
    admin's choice, shown on the binding. *Lean:* allowed for those; never from a flow's own text,
    which still needs `PrivateAddresses`.
20. **The connection's own record:** sign-ons, heartbeats, reconnects and messages no run took.
    *Lean:* a log per connection, hash-chained like a deployment's, kept as long as its runs, so
    an admin can show what passed on the link; the resource's timeline (host-resources L4) points
    into it.
21. **Bytes: hex text, or a value kind.** Hex text needs nothing new but doubles every size and
    binds every leaf to one text form; a value kind changes no existing hash (tag 8) but needs
    its boundaries settled (§5.10). *Lean:* a `Bytes` value, in step 1.1's design (§5.10),
    before the capabilities that need it.
22. **`FileServe` recording lightly** (§5.11.5). The request path's rule (C5, built) lets a
    handler be `lazy` or `unrecorded` only when every capability is sourced, so a file route
    would be `strict`, an fsync per asset. `FileServe` changes nothing, as an answer doesn't, so
    `may_record_lightly` takes it too, through a new `Capability::is_answer()`. This amends a rule
    [the request path](request-path-L4.md) owns. *Lean:* in step 1.8's second slice, with the
    request path's I-g restated as "nothing that changes anything", which it already means.
23. **`ChannelRead` as an answer too.** A subscription changes nothing either, so a subscription
    route, and a WebSocket's `open` that subscribes (§5.12.5), could be `lazy` by the same
    argument. *Lean:* yes, in its own change with its own test, since it widens what an existing
    capability allows.
24. **A flow run with no server** (`holon exec todo.holon add "buy milk"`), for a command-line
    app a person installs alone (§5.14). Either it skips approval and recording, which breaks the
    promise, or the binary carries a single-user host: a grant asked for on first run and kept in
    `~/.holon`, and a log beside it, as `holon serve` keeps one. *Lean:* not until a job asks;
    `holon do` against a local `holon serve` covers it, and the single-user host is the shape to
    take if one does. *Resolved 2026-11-07* (§5.15): `holon build` writes the flow into a copy of
    the runtime, a single-user host; the builder approves at build, the runner accepts on first
    run within that grant, and the log is kept beside the app's data, as a server keeps one.

25. **A feed's position under `lazy` and `unrecorded`** (§5.9, what a trading bot adds). *Lean:*
    the position follows the record: strict at the run's end, lazy at the next sync, unrecorded
    at the run's end. A crash delivers again what wasn't on disk, and those runs run again, since
    their claims went with their records: once per message holds for strict handlers only. The
    approval of a lazy or unrecorded feed handler says so ("after a crash, the last second's
    messages may run again").
26. **Conflation, `on_full: latest`.** Keep one waiting message per key, the newest. *Lean:* yes,
    beside `stop` and `skip`, with the key a field the binding names and `e.replaced` the count;
    `stop` stays the default (fork 17).
27. **Clock periods under a second.** *Lean:* milliseconds down to 100, the scheduler waking for
    the next due tick rather than once a second; a later change to the triggers, its own test,
    not step 1.6.
28. **New dependencies of core** (§5.1.1, slices 1.2d to 1.2f). Core has six, and its purity
    argument rests on them (`holon-core/Cargo.toml`). Inflating and deflating (gzip, zip), RSA,
    P-256, Ed25519 and OpenPGP are each too large and too easy to get wrong to write by hand.
    *Lean:* pure-Rust crates that build for wasm (`miniz_oxide`; RustCrypto's `rsa`, `p256`,
    `ed25519-dalek`; `pgp`), each pinned in `Cargo.lock` with a comment as `libm` is, with no
    feature that reads the clock or the operating system's randomness; decimals, CSV, dates and
    XML by hand. *Decided for compression* (§5.1.8): `miniz_oxide` 0.9.1, its `with-alloc`
    feature alone, pinned with its comment and a test that pins the bytes it writes; core has
    seven. *Decided for signatures* (§5.1.10): `rsa` 0.9.10, `p256` 0.13.2 and `ed25519-dalek`
    2.2.0, without their default features, so none reads the operating system's randomness, each
    pinned with its comment; core has ten. *Decided for OpenPGP* (§5.1.12): no `pgp` crate;
    `curve25519-dalek` 4.1.3, `aes` 0.8.4 and `sha1` 0.10.7, without their default features,
    each pinned with its comment; core has thirteen.
29. **PGP encryption is random.** A message is encrypted with a fresh session key, and a leaf
    must give the same answer on every replay (R1). Either the host answers it as a request, its
    ciphertext kept in the log as an answer is, or the randomness comes into the run from a
    driver, as `random` does, made bytes. *Lean:* the driver: the leaf stays pure over its inputs,
    and verify recomputes the ciphertext from the logged randomness. *Decided so* (§5.1.12): a
    run's randomness is 32 bytes, `random` read from them and `random_bytes` them.
30. **XML's shape in a flow** (§5.1.5). JsonML's lists, one form both ways, or records as
    fast-xml-parser makes them, easier to read (`doc.Document.GrpHdr.MsgId`) but losing a
    schema's order when written, and changing shape between one child and several. *Lean:* lists,
    read through `xml_find`, `xml_text` and `xml_attribute`; a record form only if reading
    turns out to be most of what flows do, and then for reading alone.
31. **Names with a namespace** (§5.1.5). Kept as written (`ns:Document`, `xmlns:ns` an attribute)
    or resolved (`{urn:…}Document`, Clark's notation). As written reads and writes back exactly
    and is what a flow writes; a sender that prefixes the same namespace differently reads
    differently. *Lean:* as written now; a path step that names the namespace (`{urn:…}Ntry`),
    and `//` for any depth, when the first statement from a bank that prefixes arrives (1.5's
    camt.053).
32. **What places an app's payload** (§5.15.8): `libsui` (`deno compile`'s; pure Rust, MIT;
    ELF, PE resources, Mach-O sections and an ad-hoc signature), or holon's own code for the three
    formats. *Lean:* `libsui`, if its size, its maintenance and its output's determinism (§5.15.9)
    hold up in slice 1; otherwise holon's own, a few hundred lines a format.
33. **The port of a built app.** The port is in the text, so in the unit's hash, and a server
    refuses two deployments on one port. Two copies of a serving app on one machine need two
    ports. *Lean:* `--port N` as a binding, like a folder's: the text's port the default, the
    runner's choice kept in the data folder and shown by `--holon`; nothing about the unit changes.
34. **Which runtime by default.** Slim is smaller and has less in it; full has the server
    database engines, which a runner may want to bind. *Lean:* slim; full with `--runtime full`,
    and `holon build` suggests it when the flow uses a database.
35. **A builder's witness the runner can't remove.** A team auditing every copy needs it to stay;
    a runner may not want their run times known. *Lean:* it stays: it is part of the grant both
    people approve, shown as such, and a runner who refuses it doesn't run the app. The runner
    may add a witness of their own.
36. **Several flows in one app,** or an app whose flow runs another's action
    (`RunsAction("billing/charge")`). *Lean:* refused until a job asks; then the payload holds
    several roots, each a deployment in the data folder, each approved on its own.
37. **Two apps under one name.** Data folders are named by flow name, so two different apps
    called `todo-cli` would share one. *Lean:* the folder records the name and the first build's
    approver, and a build approved by someone else asks before taking it over; `--data` keeps
    them apart.
38. **The approver's identity.** A name the builder states, or a signature in the payload with a
    key `holon inspect` checks against one the runner trusts. *Lean:* a name, with the file's
    platform signature as the proof of who built it; a holon signature only if teams need one
    that doesn't depend on platforms (Linux has none).
39. **Exit codes of an app.** *Lean:* 0 and 1 by the answer's status, an envelope's `exit` when
    it gives one, 2 for a command line that doesn't parse, and the BSD sysexits for the app's own
    refusals: 70 a damaged payload, 77 not accepted, 78 a secret or binding missing.
40. **A token without `exp`** (§5.1.9). jose and jsonwebtoken accept one, valid forever. *Lean:*
    refused: a bearer token that never expires is a credential nobody can take back; a flow that
    means to trust one checks its signature with `hmac_sha256` itself. *Built so* (§5.1.10).
41. **Keys beyond these** (§5.1.9). X.509 certificates (a JWK's `x5c`, a PEM `CERTIFICATE`), P-384
    and P-521 (ES384, ES512), Ed448. *Lean:* when an issuer brings one; each is another crate
    (`p384`, `x509-cert`), and the algorithms here are what the major issuers sign with.
42. **A verify that says whether a secret matched** (§5.1.9). With a shared secret, `jwt_verify`
    tells whether a guessed token's signature matches, as `secure_equals` tells whether two texts
    do. *Lean:* the taint stops, as at `hmac_sha256`: what comes out is the token's own payload.
    *Built so* (§5.1.10).
43. **A bearer token in the log** (§5.1.9). A run's log keeps its request, its `authorization`
    header included, so whoever reads the deployment's log can read a token while it is still
    valid. *Lean:* as today, the log's readers are the deployment's and `exp` bounds the token's
    use; a header kept by its hash alone is a log setting of its own, later.
44. **OpenPGP's version 6** (§5.1.11). RFC 9580's keys (X25519 and Ed25519 as their own
    algorithms, SHA-256 fingerprints) and its AEAD packet (SEIPD version 2). GnuPG reads neither
    yet; Sequoia and OpenPGP.js write both. *Lean:* when a bank's key is one, beside version 4.
45. **Other curves and old algorithms** (§5.1.11). P-256 to P-521, Brainpool, DSA and ElGamal.
    *Lean:* NIST curves when a bank brings one (`p256` is already in core; `p384` and `p521`
    aren't); DSA and ElGamal never: 1,024-bit DSA is what they mostly are.
46. **Signed messages** (§5.1.11). A message that carries its own signature (`gpg --sign`, a
    cleartext signature) rather than one beside it. *Lean:* with decrypting (step 1.5), since a
    bank's signed file is usually encrypted too and its signature is inside.
47. **Compressing before encrypting** (§5.1.11). GnuPG compresses by default. *Lean:* never:
    compression before encryption leaks lengths, and a flow that wants it zips first.
48. **SHA-1 in self-signatures** (§5.1.11). Keys made before about 2012 bind their subkeys with
    SHA-1. *Lean:* accepted in a key's own self-signatures and bindings, as GnuPG does, since a
    forged one needs the key's owner; refused in a document's signature, where a collision is the
    attack.
49. **The literal packet's file name** (§5.1.11). GnuPG writes the file's name, and some bank
    systems name what they decrypt by it. *Lean:* none, until one asks; it is advisory, and a
    leaf of its own would carry it.
50. **SMTP by hand, and how sure delivery is** (§5.2.2). `lettre` does SMTP and MIME, with some
    thirty crates and its own TLS choices; SMTP's commands and the MIME a flow sends are small and
    the host writes every byte of them. A mail the server accepted can't be taken back, and one
    whose acceptance was lost on the wire can't be known: sent again with the same `Message-ID`,
    a receiver may show it twice. *Lean:* by hand; tried again on a temporary failure, the same
    `Message-ID`, so a duplicate is rare and recognizable. *Decided so* (§5.2.3), with a spool in
    place of a delivery record: a mail's file is written before its run's record, and a restart
    sends only those whose run the log holds.
51. **Bcc and Reply-To** (§5.2.2). A blind copy hides a recipient from the others, and Reply-To
    sends the answers elsewhere: both are what a phishing mail uses. *Lean:* neither until a flow
    needs one; then each held to the binding's `--to` as `to` is. *Decided so* (§5.2.3): a
    message with either has no value.
52. **The host's own address** (§5.2.2). A mailed link needs an address the world reaches, and
    the host knows only where it listens. *Lean:* the flow writes its base as text until a
    setting names it (`--public-url`), shared with OAuth's callback (fork 2). *Decided so for
    now* (§5.2.3): the sample writes `const site`.
53. **Mail sent as a run parks** (§5.2.2). A waiting run's channel packs go out as it parks; its
    mail could too, so the run that waits mails its own link (`resume_token`). *Lean:* when a
    sample needs it; the route that starts the run knows its token today. *Decided so for now*
    (§5.2.3): the sample's route mails the link.
54. **MIME read by hand** (§5.2.4). `mail-parser` reads more of what is out there: every charset
    `encoding_rs` knows, and the malformed mail a mail server has met. Reading MIME is a few
    hundred lines beside where core writes it, and in core the editor's replay reads a message as
    the host does. *Lean:* by hand, four charsets; a crate when mail in another one turns up.
55. **Whose verdict** (§5.2.4). The host could check DKIM itself, but the key is a DNS lookup
    outside the run, and SPF needs the address that connected, which IMAP never sees; the
    mailbox's own server decided both as the mail came in. *Lean:* that server's verdict, named
    by its `authserv-id`, none believed by default.
56. **IDLE** (§5.2.4). IMAP's `IDLE` says at once that a message came; a look every minute is
    simpler, and works with every server. *Lean:* looking, `--every`; `IDLE` when a flow needs
    seconds.
57. **A failed run's message** (§5.2.4). Moved as any other, left in the folder for another try,
    or moved to a folder of its own. *Lean:* moved, as a file whose run failed isn't run again:
    its record says why; a `--failed` folder when someone needs to tell them apart in the mailbox.
58. **A feed's runs one at a time, or several at once** (§5.9.1). One at a time keeps the feed's
    order and makes the position one id; several at once go faster but finish out of order, so
    the position is the oldest still running, and a crash runs again whatever finished after it.
    *Lean:* one at a time; `--parallel N` when a job asks.
    *Decided* (2026-11-09) as the lean.
59. **The connection's own record, now or with the socket** (§5.9.1). *Lean:* feeds and queues
    put what their connection did on the deployment's timeline (connects, reconnects, skipped
    messages), as a mailbox's looks are; fork 20's record comes with the socket, whose sign-ons
    and heartbeats need one.
    *Decided* (2026-11-09) as the lean.
60. **A queue message whose run failed** (§5.9.1). Deleted, as a mail is moved, or left to come
    back and run again until the queue's redrive policy moves it aside. *Lean:* deleted, so one
    bad message doesn't run forever, its record saying why; `--keep-failed` leaves it for a queue
    that has a redrive policy.
    *Decided* (2026-11-09) as the lean.
61. **`on.feed` and `on.queue`, or `on.event` and `on.message`** (§5.9.1). *Lean:* named for
    what they listen to, as `on.mail` and `on.file` are; *message* stays the socket's.
    *Decided* (2026-11-09) as the lean.
62. **`latest` replaces whenever its key waits, or only once the buffer is full** (§5.9.4).
    Only once full, a slow handler first works through `--buffer` stale prices. *Lean:* whenever:
    a price that waits while a newer one comes is stale, full or not. The buffer then holds one
    message a key, and a new key waits for room.
63. **Which messages run: filtered by the binding (`--only`) or by the handler** (§5.9.4). The
    messages a flow doesn't want are mostly the answers to the binding's own subscription and
    heartbeat. *Lean:* the binding, which writes what they answer; a handler's own filter waits
    for a job.
64. **A WebSocket feed resumed from its position** (§5.9.4). *Lean:* not in 1.6b. The feeds
    a job brings send a snapshot on subscribe, not a replay, and the position already stops a
    message running twice. A feed that resumes from a sequence would get it written into its
    subscription (`{position}`) when a job asks.
    *Decided* (2026-11-09) otherwise: asked how a feed resumes properly, the owner took a resume
    where the binding says (`--resume`, 1.6c), and the gaps of a feed that can't replay
    (`--sequence`, 1.6d), after 1.6b (§5.9.6).
65. **The trading bot's order half in 1.6b** (§5.9.4): the order a strict action, its client id,
    approval over a limit, reading the exchange's orders back. None of it is new to 1.6b, and the
    language has no loop that waits on a call until the TypeScript slices 6 to 11. *Lean:* in
    1.6b, kept to what the language has: one order a run, the orders read back as one value.
    §5.9 promises step 1.6's sample shows it.
66. **A live run against a public feed** (§5.9.4): Coinbase Exchange's ticker, which needs no
    account, for the prices; the orders to the stand-in. *Lean:* yes; it is an outside service
    with no credentials to keep.
67. **`--resume` and an id of several fields** (§5.9.6). The position `BTC-USD/41` is no cursor.
    *Lean:* refused: a feed that resumes takes one value, and a binding that needs two writes them
    in a `message:` the host can't build from one.
68. **`--resume message:` in place of the subscription, or after it** (§5.9.6). *Lean:* in place:
    a feed that resumes by message takes the position in its subscription (Nostr's `REQ` with
    `since`, a GraphQL subscription's variables); sending both would subscribe twice.
69. **A gap counted as the message is read, or as it runs** (§5.9.6). As it runs, `latest` and
    `skip` would make gaps the feed never made. *Lean:* as it is read, the feed's gaps only, a
    replaced message's gap carried by the one that replaced it.
70. **A message `--only` passes over counted in the sequence** (§5.9.6). *Lean:* counted: the feed
    numbered it, and a flow that filters a `full` channel to its matches would otherwise see a gap
    at each message of another type.
71. **A number not above the last** (§5.9.6): a message sent again, or a count begun anew on a new
    connection. *Lean:* `e.gap` 0, and the number kept as the last: a duplicate with an id is
    passed over anyway, and a feed that numbers per connection says nothing of what came between
    connections, which no field can mend.
72. **How many keys' numbers are kept** (§5.9.6). *Lean:* 10,000, the least recently read
    forgotten: a feed of every symbol on an exchange fits, and a key forgotten only loses one gap.
73. **Two slices and two samples** (§5.9.6): `--resume` with `bluesky-mentions`, `--sequence` with
    `trade-tape`, a commit each. *Lean:* yes: neither needs the other, and no one feed shows both
    (Jetstream's `seq` skips the collections it filters out; Coinbase's trades can't be replayed).
74. **Live runs against Jetstream and Coinbase Exchange's trades** (§5.9.6): public, no account.
    *Lean:* yes.
75. **Which AWS credentials** (§5.9.9). A key id and secret in the URL, with a session token for
    temporary keys; or the credential chain AWS's SDKs read (environment, profile, an instance's
    role). *Lean:* the URL, as every other binding's login is, read from a source each time; the
    chain when a job runs on AWS and has a role to use.
76. **SQS's JSON protocol or its query protocol** (§5.9.9). The query protocol (form-encoded, XML
    answers) is older and every copy speaks it; JSON is what AWS's SDKs send now. *Lean:* JSON:
    serde already reads it, and ElasticMQ and LocalStack answer it.
77. **How a queue's send is tried again** (§5.9.9). Mail waits a minute, then 5, 30, an hour: a
    person reads it. A queue's reader is a program waiting on it. *Lean:* 1 s, 10 s, a minute,
    then every 5 minutes, for a day.
78. **A standard queue's send tried again** (§5.9.9): SQS has no deduplication there. The host
    could add an attribute with the run and the send, for listeners to check. *Lean:* no:
    at-least-once is a standard queue's contract, and a FIFO queue is the one that dedupes;
    the guide says so.
79. **The sample** (§5.9.9): `payout-events`, a provider's events in, a FIFO queue of retries out:
    one flow listens and sends. *Lean:* yes: the use payment ops have, and it shows both halves.
80. **The live run's queue** (§5.9.9): AWS SQS needs the owner's account and a key; ElasticMQ
    runs here, but is a copy. *Lean:* AWS, if the owner makes the key; ElasticMQ otherwise.

## 9. What stays true

- A flow names things; the host decides where they are.
- Every item the approver reads names the thing at the grain consent needs.
- A run's input is in its log: a replay and `holon verify` need no outside system.
- Effects land when the run finishes, at most once.
- Nothing computed from a secret is kept or logged, except through a one-way leaf; a key the
  host holds never enters a run.
- No code runs outside a boundary the manifest describes: a program as an admin bound it, or
  anything inside a sandbox whose walls the operating system holds. Nobody gets a shell with the
  server's reach, and no sandbox is offered where its walls can't be held.
- Every capability ships with a sample deployed by the e2e tests against a local stand-in.

## 10. Where each design is

**Here, from 2026-11-06.** Each step's design is its section of this note, written when the step
starts: the section §7 names grows numbered subsections answering §4's checklist (what is
already there, the record and its limits, the checklist, forks, samples and slices), as §5.11
and §5.12 did. A capability no section covers yet gets the next one of §5. Its forks join §8, numbered on.
A section is cited as `host-capabilities §5.9` or by its link, `host-capabilities-L4.md#59-…`.

| Steps | Design |
|---|---|
| 1.1 value kinds | §5.10 |
| 1.2 formats | §5.1 |
| 1.3 email | §5.2 |
| 1.4 remote folders | §6 |
| 1.5 secure connectivity | §5.3, §5.4 |
| 1.6 messaging (feeds, queues) | §5.9, §5.5 |
| 1.7 commands | §5.7 |
| 1.8 static files | §5.11 (built) |
| 1.9 inbound WebSockets | §5.12 (designed) |
| 2.1 approvals by role | §5.6 |
| 2.2 telemetry and audit export | a new section of §5 |
| 2.3 who touched what | [host resources](host-resources-L4.md), revised there |
| 2.4 a sandbox for agents | §5.8 |
| 2.5 history | [history](history-L4.md) (designed) |
| Tier 3 | §5.13, §5.14, §5.15 (`holon build`, designed) and new sections, as they are asked for |

**In L4s of their own, designed before then,** each a host capability this note builds on:
[named folders](named-folders-L4.md), [databases](databases-L4.md),
[channels](channels-L4.md), [triggers](triggers-L4.md),
[host resources](host-resources-L4.md), [secret destinations](secret-destinations-L4.md),
[storage](storage-L4.md) and [history](history-L4.md). They stay where they are: other docs and
the code cite them by file, and each is the record of its own design. A change to one of those
capabilities is designed in its own L4, as before.
