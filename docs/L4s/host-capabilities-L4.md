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
flow as a command-line app (§5.14). It
sets rules and sorts candidates. Most capabilities it says to build get their own L4, as folders,
databases and channels did; the web's are designed here, so the theme is in one place. Builds on
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
delivery is at least once and the message id, kept in the claims table, makes it once. One engine
comes in Tier 1 (§7, step 1.6), which one is §8's fork 8: SQS shares S3's request signing (§6),
RabbitMQ and NATS have pure-Rust clients, Kafka's mature client is C (librdkafka).

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

**Engines.** The data plane already runs on tokio, so a WebSocket client is tokio-tungstenite on
that runtime, and inbound WebSockets' server side is the same crate. SSE is a streaming read the
outbound client can do. A Postgres `LISTEN` is the
`postgres` crate's notifications, beside `engines.rs`. MQTT is rumqttc. gRPC (tonic, HTTP/2 and
protobuf) waits for a job that brings one. A socket's framing and session are Holon's own code,
small; the protocols' meaning stays in leaves.

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

1. **The rest segment:** core, language, printer, bridge, editor matcher, docs; the `site` sample
   without `serve` (an index page from a `js` holon, a catch-all answering it).
2. **`file_serve`:** the primitive, the capability and its description, `respond`, the
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
Running a flow with no server at all is fork 24.

**Slices.** (1) `holon do`, its argument mapping, output, exit codes and `--help`; `holon command`;
`@file` and `-o`; a sample, `todo-cli`, deployed by the e2e tests and driven by `holon do` in a
test. (2) Actions that resume, and `holon do` answering a question. (3) Progress, once actions'
replies stream. (4) `holon attach`, after step 1.9. Not scheduled in §7: a step when it is asked
for.

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

- **Its L4 first,** answering §4's checklist, reviewed before code. Then slices, each one commit.
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

| Step | What | L4 | Needs | Size |
|---|---|---|---|---|
| 1.1 | **Value kinds:** `Bytes`, and `Decimal` as the L4 decides (§5.10) | value-kinds-L4 | — | M |
| 1.2 | **Formats:** exact decimals, CSV, XML, dates (UTC and offsets), zip and gzip, JWT verify, PGP encrypt and verify with a public key (§5.1) | payment-formats-L4 | 1.1 | M |
| 1.3 | **Email:** send, recipients in the grant; receive as `on mail`, with its sender's verdict and attachments (§5.2) | mail-L4 | 1.1 for attachments | L |
| 1.4 | **Remote folders:** the backend seam in `folders.rs`, then SFTP and S3-compatible storage (§6) | folder-backends-L4 | 1.1 for binary files | L |
| 1.5 | **Secure connectivity:** keys the host holds (PGP decrypt and sign) and client identities (mutual TLS, a private CA) (§5.3, §5.4) | bank-connectivity-L4 | 1.1, 1.2 | M |
| 1.6 | **Messaging:** one subscription runtime (position, once per message, backpressure, reconnect), feeds (SSE, WebSocket, Postgres `LISTEN`) and one queue engine, publish and consume (§5.5, §5.9) | streams-L4 | 1.4's request signing, for SQS (fork 8) | L |
| 1.7 | **Commands:** programs an admin binds, typed arguments, logged output, killed on timeout (§5.7) | commands-L4 | — | M |
| 1.8 | **Static files:** a route's rest segment (`/*path`, routing, no item), and answering with a file from a folder (`site.serve(name)`, `FileServe`), streamed by the host and checked against the handle's hash | §5.11 (here) | — | M |
| 1.9 | **Inbound WebSockets:** `websocket PATH { open … message … close … }`, a run per message, replies and channel packs out, a record per connection (§5.9) | §5.12 (here) | 1.1 for binary frames | M |

- **Samples, one at least per step:**
  - 1.1 `file-checksums` (a binary file's hash, into a manifest);
  - 1.2 `payment-batch` (a CSV of payouts in, a pain.001 out, its control sum exact);
  - 1.3 `approve-by-email` (a payout over a limit mails an approver its resume link) and
    `invoice-inbox` (an invoice's attachment into a folder);
  - 1.4 `bank-drop` (statements arriving over SFTP, into a collection);
  - 1.5 `bank-statements` (SFTP, PGP decrypt, camt.053 parsed);
  - 1.6 `price-feed` and `payout-events`;
  - 1.7 `nightly-reconcile` (a bound program on a clock, its output into a folder);
  - 1.8 `site` (a built web app and its API, with a deep link falling back to `index.html`);
  - 1.9 `room` (a chat: joins, messages and leaves over one connection per browser).
- **Stand-ins for e2e:** an SMTP sink and an IMAP server, an SFTP server and an S3 stub, an HTTPS
  server that requires a client certificate, an SSE and WebSocket stub, a test program; each
  started by the e2e setup. 1.8 and 1.9 need none: a fixture folder, and the browser as the
  client. A queue engine that can't be stubbed in-process (a real broker) is
  settled in streams-L4.
- **Alongside 1.7:** retire `egress` and `Spawn` from `Capability` and the catalog (§8, fork 7),
  `CommandRun` taking `Spawn`'s place. S.

**Tracks.** 1.1 is the one most others stand on, so it starts first. Then:

```
A  1.1 ──► 1.2 ──► 1.5
B  1.4  (text files at once; binary files once 1.1 lands)
C  1.3  (text mail at once; attachments once 1.1 lands)
D  1.6, 1.9  (one engine, tokio-tungstenite; 1.9's text frames at once, binary once 1.1 lands)
E  1.7
F  1.8  (the rest segment, then FileServe)
```

Six tracks, so six worktrees at most; tracks that touch the same files (`kind.rs`, the catalog,
`tests.rs`) merge into `main` one at a time, each rebased on the last.

### Tier 2: operationally essential

| Step | What | L4 | Needs | Size |
|---|---|---|---|---|
| 2.1 | **Approvals by role:** `await approval(role: …)`, the editor's inbox, the approver in the log, four eyes, *n* of *m* (§5.6) | approvals-L4 | — | M |
| 2.2 | **Telemetry and audit export:** OpenTelemetry over HTTP (a trace per run; metrics for runs, failures, latency, a feed's or queue's lag), the activity log and run heads to a SIEM, health and readiness endpoints (§2's server features) | telemetry-L4 | 1.6 for lag | M |
| 2.3 | **Who touched what:** resources with ids, addresses and a timeline across deployments ([host resources](host-resources-L4.md), designed), and each connection's own record (§8, fork 20) | host-resources-L4, revised | 1.4, 1.6 | L |
| 2.4 | **A sandbox for agents:** walls from the grant, the operator's container runtime first, then Linux's own; the outbound proxy; writes at commit; agents' shell through MCP (§5.8) | sandbox-L4 | 1.7 | L |
| 2.5 | **History:** one timeline per deployment of its runs, deploys and grant changes, and restoring its code, its state or both to any earlier point, as a new entry ([history](history-L4.md), designed) | history-L4 | 2.1 for approved restores | M |

Samples: 2.1 `four-eyes-payout`; 2.4 `agent-reconcile` (an agent's script in the sandbox,
reading statements, writing a report); 2.5 `price-rollback`. 2.2 and 2.3 are the server's, shown in the server manual
and checked by e2e tests of their own.

If a first outside job comes before Tier 2, **2.1 is the step to pull forward**: it needs nothing
from Tier 1, and it is the capability closest to the pitch.

### Tier 3: nice to have

Taken when a job or a user asks, each with its L4 and its sample all the same:

- **Persistent terminals** (a pseudo-terminal, SSH) and WebSocket sessions (§5.7, §5.9).
- **Sockets for payment networks:** ISO 8583, FIX, SMPP (§5.9). Promoted to Tier 1 if the first
  job runs on a card processor's link.
- **More engines:** the queues 1.6 didn't take (Kafka, NATS, SQS or RabbitMQ), MQTT, gRPC streams.
- **Mailboxes through OAuth:** Microsoft Graph, the Gmail API (§8, fork 2).
- **Stronger or wider sandboxes:** a micro-VM, TLS inspection at the proxy, a WASI backend
  (§8, forks 15 and 16).
- **The rest:** time zones (fork 3), PDF output, SMB and Azure Blob folder backends, keys in a
  KMS or an HSM, a model resource with a budget.

## 8. Forks, surfaced

1. **Decimals as text leaves or a value kind.** A `Decimal` value, like `Bytes`, takes a new tag
   (no existing hash moves) but needs its JSON form, its place in the log, the bridge, the editor
   and every leaf that takes a number; leaves over text need none of that. *Lean:* decided in
   value-kinds-L4 with bytes, whose boundary answers make a `Decimal` value cheap; text leaves if
   that L4 finds a reason against.
2. **Mailboxes that need OAuth.** Microsoft 365 has turned passwords off for IMAP and SMTP, and
   Google steers to OAuth. Either the host does OAuth (a refresh token as the secret, the host
   refreshing it), or those mailboxes are reached through their HTTP APIs (Microsoft Graph, Gmail
   API) as other backends.
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
   *Lean:* retire `egress` and `Spawn` once nothing uses them (`CommandRun` takes `Spawn`'s
   place, §5.7); `SecretAccess` is the coarse form `SecretRead` replaced.
8. **Which queue first** (§7, step 1.6). SQS's API is HTTPS, so its e2e stand-in is an
   in-process stub and its signing is S3's (step 1.4); RabbitMQ is what self-hosted shops run,
   but its stand-in is a real broker in CI. *Lean:* SQS-compatible first, RabbitMQ in Tier 3.
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
    its boundaries settled (§5.10). *Lean:* a `Bytes` value, in value-kinds-L4, before the
    capabilities that need it.
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
    take if one does.

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

## 10. The L4s that follow

One per step of §7, written when its step starts: value-kinds, payment-formats, mail,
folder-backends, bank-connectivity, streams and commands for Tier 1; approvals, telemetry, a
revision of host-resources, sandbox, and history (already written) for Tier 2; Tier 3's as they
are asked for. Steps 1.8 and 1.9 have none: static files and inbound WebSockets are designed in
this note (§5.11, §5.12), and what else the web asks for is sorted beside them (§5.13).
