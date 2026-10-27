# Host capabilities: what earns one, and what Holon already covers (L4)

**Status:** design, before any code. Written 2026-10-27, against rc.27 (1ccf376), and the
triggers design as of 2befd26. It sets rules and sorts candidates; each capability it says to build gets its
own L4, as folders, databases and channels did. Builds on
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
- by hand: `ingress`, run through `/trigger`. **Actions** (a person, an agent or another flow,
  through Holon) are designed in the triggers L4, not built.

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

**Three capabilities nothing in the language writes:** `NetworkEgress` (the `egress` leaf: the
host records it as performed and sends it nowhere; no sample uses it), `SecretAccess` and
`Spawn` (`kind.rs`).

**Numbers are 64-bit floats** (`holon-core/src/value.rs`, `Number(f64)`). Integers are exact to
2^53, so amounts in minor units (cents) are safe; `"1234.56"` parsed and summed is not.

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

**R4. Earning a capability doesn't schedule it.** `PLATFORM_LAYOUT.md` §7: a feature no step
toward M3 needs waits. The rules say *how* something would be built, the job says *when*.

**R5. Never a capability that runs code the manifest can't describe.** `Spawn` (starting a
process) would put anything at all behind one item, outside replay and outside the grant. It
stays out of the language. A business process that must run a program does it behind an HTTP
endpoint the operator owns.

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
| `Spawn` | never | R5 | |
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
  not.
- **Dates**: parse and format ISO 8601, add days, the weekday; business days given a list of
  holidays (the list is data, not built in). Time zones need the tz database in the binary
  (§8, fork 3).

MT940, BAI2 and NACHA come after, as samples built from text leaves (or a fixed-width leaf), not as
leaves of their own: there are too many formats to put each in the catalog.

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
delivery is at least once and the message id, kept in the claims table, makes it once. Which
engine first is the job's choice: SQS shares S3's request signing (§6), RabbitMQ and NATS have
pure-Rust clients, Kafka's mature client is C (librdkafka).

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

## 7. In what order, for payment operations

For M3's job (payment operations at a fintech), and only as the job confirms it:

1. **The four leaves** (§5.1): decimals, CSV, XML, dates. No permissions, no engines, and they
   unblock every payment file. Sample: `payment-batch`, a CSV of payouts in, a pain.001 out to a
   folder, its control sum exact.
2. **Approvals by role** (§5.6): what the pitch is about.
3. **Email** (§5.2): sending with its recipients first, receiving second.
4. **Bank connectivity, together:** the SFTP backend (§6), keys for PGP (§5.3) and client
   certificates (§5.4). A bank needs all three; any one alone connects to nothing.
5. **Then what the job asks:** object storage, a queue, telemetry.

## 8. Forks, surfaced

1. **Decimals as text leaves or a value kind.** A `Decimal` value would change units' hashing,
   the wasm bridge and every leaf that takes a number. Leaves over text are additive.
   *Lean:* text leaves now; a value kind only if flows show they need it.
2. **Mailboxes that need OAuth.** Microsoft 365 has turned passwords off for IMAP and SMTP, and Google steers to OAuth.
   Either the host does OAuth (a refresh token as the secret, the host refreshing it), or those
   mailboxes are reached through their HTTP APIs (Microsoft Graph, Gmail API) as other backends.
   *Lean:* SMTP and IMAP with passwords first (most providers and relays take them), Graph as the
   second backend if the job is on Microsoft 365.
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
   *Lean:* retire `egress` and `Spawn` once nothing uses them; `SecretAccess` is the coarse form
   `SecretRead` replaced.
8. **Which queue first.** *Lean:* none until a job names one.

## 9. What stays true

- A flow names things; the host decides where they are.
- Every item the approver reads names the thing at the grain consent needs.
- A run's input is in its log: a replay and `holon verify` need no outside system.
- Effects land when the run finishes, at most once.
- Nothing computed from a secret is kept or logged, except through a one-way leaf; a key the
  host holds never enters a run.
- A flow runs no code the manifest can't describe.
- Every capability ships with a sample deployed by the e2e tests against a local stand-in.

## 10. The L4s that follow

In §7's order, each written when its turn comes: **payment-formats-L4** (the four leaves),
**approvals-L4**, **mail-L4**, **folder-backends-L4** (SFTP first, with keys and client
identities as **bank-connectivity-L4**), then object storage, queues and telemetry as the job asks.
