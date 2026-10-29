# Host capabilities: what earns one, and what Holon already covers (L4)

**Status:** design, before any code. Written 2026-10-27, against rc.27 (1ccf376), and the triggers
design as of 2befd26. Revised 2026-10-29 against rc.29 (6bc5c68): commands and persistent
terminals (§5.7), a shell in a sandbox whose walls are the grant (§5.8), outbound streams and
long-lived connections (§5.9), a bytes value with decimals decided beside it (§5.10), R5 reworded
around boundaries, and the implementation plan in three tiers (§7, replacing the order for payment
operations), with history as step 2.5 ([history](history-L4.md)). It sets rules and sorts candidates; each capability it says to build gets its own L4,
as folders, databases and channels did. Builds on
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
| ISO 8583, FIX, SMPP, a bank's own socket | host capability: a **socket** | R3a, R3c, R3d | §5.9; when a job brings one |
| inbound WebSockets | later | — | channels and a POST cover browsers |
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

**Inbound WebSockets** aren't needed yet: channels (SSE) and a POST cover what a browser sends and
hears. They wait for a job with two-way traffic from a browser.

**Engines.** The data plane already runs on tokio, so a WebSocket client is tokio-tungstenite on
that runtime. SSE is a streaming read the outbound client can do. A Postgres `LISTEN` is the
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

- **Samples, one at least per step:**
  - 1.1 `file-checksums` (a binary file's hash, into a manifest);
  - 1.2 `payment-batch` (a CSV of payouts in, a pain.001 out, its control sum exact);
  - 1.3 `approve-by-email` (a payout over a limit mails an approver its resume link) and
    `invoice-inbox` (an invoice's attachment into a folder);
  - 1.4 `bank-drop` (statements arriving over SFTP, into a collection);
  - 1.5 `bank-statements` (SFTP, PGP decrypt, camt.053 parsed);
  - 1.6 `price-feed` and `payout-events`;
  - 1.7 `nightly-reconcile` (a bound program on a clock, its output into a folder).
- **Stand-ins for e2e:** an SMTP sink and an IMAP server, an SFTP server and an S3 stub, an HTTPS
  server that requires a client certificate, an SSE and WebSocket stub, a test program; each
  started by the e2e setup. A queue engine that can't be stubbed in-process (a real broker) is
  settled in streams-L4.
- **Alongside 1.7:** retire `egress` and `Spawn` from `Capability` and the catalog (§8, fork 7),
  `CommandRun` taking `Spawn`'s place. S.

**Tracks.** 1.1 is the one most others stand on, so it starts first. Then:

```
A  1.1 ──► 1.2 ──► 1.5
B  1.4  (text files at once; binary files once 1.1 lands)
C  1.3  (text mail at once; attachments once 1.1 lands)
D  1.6  (feeds at once; the SQS engine once 1.4's signing lands)
E  1.7
```

Five tracks, so five worktrees at most; tracks that touch the same files (`kind.rs`, the catalog,
`tests.rs`) merge into `main` one at a time, each rebased on the last.

### Tier 2: operationally essential

| Step | What | L4 | Needs | Size |
|---|---|---|---|---|
| 2.1 | **Approvals by role:** `await approval(role: …)`, the editor's inbox, the approver in the log, four eyes, *n* of *m* (§5.6) | approvals-L4 | — | M |
| 2.2 | **Telemetry and audit export:** OpenTelemetry over HTTP (a trace per run; metrics for runs, failures, latency, a feed's or queue's lag), the activity log and run heads to a SIEM, health and readiness endpoints (§2's server features) | telemetry-L4 | 1.6 for lag | M |
| 2.3 | **Who touched what:** resources with ids, addresses and a timeline across deployments ([host resources](host-resources-L4.md), designed), and each connection's own record (§8, fork 20) | host-resources-L4, revised | 1.4, 1.6 | L |
| 2.4 | **A sandbox for agents:** walls from the grant, the operator's container runtime first, then Linux's own; the outbound proxy; writes at commit; agents' shell through MCP (§5.8) | sandbox-L4 | 1.7 | L |
| 2.5 | **History:** runs, deploys and grants as one chain with state roots, a diff of a new version against recorded runs before approval, reverts of state and code as new links, compensating actions ([history](history-L4.md), designed) | history-L4 | 2.1, 2.3 | L |

Samples: 2.1 `four-eyes-payout`; 2.4 `agent-reconcile` (an agent's script in the sandbox,
reading statements, writing a report); 2.5 `payout-revert` and `notes-history`. 2.2 and 2.3 are the server's, shown in the server manual
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
- **Inbound WebSockets.**
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
revision of host-resources, sandbox, and history (already written) for Tier 2; Tier 3's as they are asked for.
