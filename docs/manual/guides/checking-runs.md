# Checking runs

Every run of a deployment is kept in its run log, with everything it read from outside and
everything it did. This guide shows how to export a deployment's runs and check them on another
machine, and how a witness makes the log tamper-evident. [Concepts](../concepts.md#the-run-record)
explains why this works.

## The chain

Each entry of the log includes the hash of the entry before it; the newest entry's hash is the
log's **head**. Change, remove or insert a run afterwards, and the hashes no longer line up.

The editor checks the chain as it shows the log. In view mode, above the run list:

- **chain unbroken**, or **✗ chain breaks at run N: changed outside holon**;
- **head** and its first twelve hex digits, with **copy**.

A deployment keeps its newest 500 runs by default (`holon serve --keep-runs N`). Older ones are
trimmed, and the chain is checked from the oldest kept run. How many, and for how long, is a
deploy setting of its own: in the editor's Settings, **runs the log keeps** and **days the log keeps runs**, or
the flow's `keep` line, which they follow (`keep 5 runs for 1d`, `keep 500 runs`, `keep runs for
30d`). Fewer than the host's default is the author's to choose; more asks for approval, as it is
the host's disk (the sample **rolling-log** keeps 5, a day at most). The states a trimmed run
left stay in the content store until a sweep removes the ones nothing kept names any more: the
server's own, every ten minutes, an hour after they were put, or an admin's `holon store sweep`
(`--dry-run` first) at any time.

What was done to the deployment (its [events](deploying.md#the-timeline): deploys, restores, grant
changes, secrets, bindings, pausing and running) is a chain of its own, kept for good, so it is
checked from the first event.

## Exporting

From the editor: **export runs** above the run list saves `<slug>-runs.json`.

From the command line, on the machine where the server runs:

```sh
holon export notes-api
```

```
wrote /home/you/notes-api-runs.json: 42 runs of "notes-api" and its 3 events, head 3f9c2a7be01d
```

The file holds the run log and the events exactly as written and every unit the runs ran on, so
it needs nothing else to be checked. Options:

| Option | |
|---|---|
| `--to FOLDER` | where to write it (default: here) |
| `--as NAME` | its name (default `<slug>-runs.json`) |
| `-o PATH` | the whole path, in place of `--to` and `--as` |
| `--server PORT\|URL` | which server: with several running here, a port; a server elsewhere, its URL, with an API token in `HOLON_TOKEN` |
| `--max-files-bytes N` | ship at most `N` (`64K`, `10M`, `2G`) of the file texts runs read, the oldest runs' first |
| `--without-files` | ship none of them |
| `--run N` | run N alone, to `<slug>-run-N.json` |

**One run** (`--run N`, or **Check offline…** on a run's page): its line exactly as written, which
names the hash of the line before it, and the units and files it ran and read. `holon verify` runs
it again to its entry and checks its hash, and says the hash of the line before it, which only the
log has. Whether that line is the one a witness holds a head after takes the whole export.

## Verifying

On any machine with `holon`, with no server and no network:

```sh
holon verify notes-api-runs.json
```

```
notes-api: 42 runs, runs 1 to 42
  chain: unbroken
  head: 3f9c2a7be01d is the newest run's
  events: 3, chain unbroken from the first
  held by the chain alone, not by running each run again (a witnessed head holds it too): when it began, how long it took, when each step began, when each step ended, its run session, which load of the state it committed to, its commit's number, how many times it ran again before its commit landed (its tries), who called it (the server's account of its caller), the grant it ran under (by its hash)
  re-ran 42 runs: each produced what its log entry says, 3 of them given what the log kept for a step (named below)
  3 runs: 4, 17, 30 checked given what the log kept for 3 nodes fed a secret, a token or a tainted value (not computed again):
    hmac_sha256 fed the secret stripe_key v3 #a1b2c3d4: 3 nodes in 3 runs: 4, 17, 30, as recorded
  2 runs: 9, 31 checked given the run id the log kept (made from a secret or a resume token the run received; not computed again)
  secrets: 38 outbound requests carried one, each to a host its approval shows it going to
  tokens: 12 outbound requests carried what a call answered to a secret, each to a host its approval shows that going to
  state: run 1 began from nothing, as a log's first does; 41 runs began from the state the run before them left, 42 left what their writes make of the one they began with
ok
```

`verify` checks that:

1. the chain is unbroken, the head is the newest run's, and no run is missing. A run whose line
   the server couldn't write (a full disk) isn't a break: the line after it says so, and the
   report lists it, `run 17 isn't in the file: the server couldn't write its line, and the line
   after says so`. The server numbers runs one after another, so runs missing that no line says
   were lost were removed, and the lines after them chained again: `runs 5 to 7 MISSING`;
2. every line is one the server writes: a kind of run there is, with that kind's fields (a
   request's method, URL and status; what was delivered only on a resumed part's), each value one
   a run holds, each commit numbered once, and a numbered commit naming the state it left. One
   that isn't says why: `run 7 ISN'T A LINE THE SERVER WRITES: …`;
3. every unit in the export is what its hash says;
4. every run, run again from its log entry through the same core, produced what the log says:
   the same outputs, effects and status, the same answer (its body, as the line keeps it), the
   same steps in the same rounds (its timeline: what each step gave, or why it gave nothing), and
   the same points it waits at. Outbound calls are answered from the log, so nothing is sent. A
   run that failed is run again too: what it computed is checked, what its writes made of its
   state, and its status, unless that is its failure's. What a resumed run was delivered is held
   to what it waited for: an approval's decisions, taken in the order they came, are ones its
   steps take, each on a step there is and open as it came, by someone who decided no other step
   of it, of the step's kind and role, and whoever started the run only on a step that takes them;
5. each run's state follows from the one before: it began from the state the run before it left,
   and left what its writes make of the state it began with (see
   [The state, run to run](#the-state-run-to-run));
6. every outbound request that carried a secret (the log marks where it stood:
   `‹secret:stripe_key v3 #…›`) went to a host the flow's unit sends that secret to: what its
   approval showed ("read the secret "stripe_key", and send it to https://api.stripe.com"). A
   request the host refused before it left went nowhere. So did every request that carried a
   token a call answered to one (`‹token:answer access_token #…›`): to a host the approval shows
   what is answered to that secret going to ("…; what is answered to it goes to
   https://api.example.com").

It exits 0 when everything checks, every run was run again and every run's state checked, and 1
when anything doesn't
check, with what failed:

```
  chain: BROKEN at run 17: changed, removed or inserted outside holon
  run 23 DIFFERS: …
  run 26 SENT the secret "stripe_key" to hooks.example.com, where its approval doesn't show it going
  run 27 SENT a token (answer access_token) to hooks.example.com, where its approval doesn't show what is answered to its secret going
  run 30's state DOESN'T FOLLOW: it began from a state the run before it (run 29) didn't leave, nor a run beside it
FAILED
```

When nothing fails but some runs couldn't be run again (see below), or a run's state couldn't be
checked ([The state, run to run](#the-state-run-to-run)), it exits 3: nothing in the export was
found changed, and not all of it was checked, so a script reading the exit code doesn't take it
as checked. The report says which runs, and why:

```
  not re-run (1 run: 12): it read inbox/sales.csv, whose text the export doesn't hold (its folder keeps hashes, the server no longer keeps it, or the export left it out): give it with --files
INCOMPLETE: nothing here fails, but not every run was run again (said above)
```

### What only the chain holds

What `holon verify` checks without a witness is everything a run makes: run again, it computes
what it did, answered, waited for and left, and every line must be one the server writes. What
only a witness holds is the rest of a line, the server's account of when, where and for whom it
ran, which no run again can make. Someone who holds the server's data can change those fields and
chain the lines after them again: without a witnessed head that passes, and with one it fails
([Witnesses](#witnesses)). Without `--head`, the report says so (`held by the chain alone, …`).

| Field | What it is | Why running the run again can't make it | What holds it |
|---|---|---|---|
| `at_ms`, `duration_ms` | when the run began, and how long it took | the clock is the server's, read as it ran | the chain, and so a witness |
| the timeline's `start_us`, `end_us` | when each step began and ended | the same: a step's time is the server's | the chain, and so a witness |
| `session` | the run session it ran in | the deployment's own count of each time it was started or restored, which no run sees | the chain, and so a witness |
| `commit`'s `cell` and `n` | which load of the state it committed to, and its commit's number | they number commits as they landed beside other runs; they say which run's state it began after, so changed, `verify` may check less of its state, and says which runs it didn't | the chain, and so a witness |
| `retries` | how many times it ran again before its commit landed | a try that was overtaken leaves nothing but the answers it got, which the last try is given | the chain, and so a witness |
| `by` | who called it | the caller is the server's account: nothing in a run makes it | the chain, and so a witness; an approval's decisions are held to it (below) |
| `grant` | the grant it ran under, by its hash | the export holds the units the runs ran, not the grants | the chain, and so a witness |

`by` is only the server's account of its caller: nothing in the run makes it, and nothing in an
export can check who sent a request. An approval's decisions are held to it, as their run's
starter (whoever started a run decides only a step that takes them), but that it was they who
started it is the server's word. So is who each person deciding is, and whether the role binding
each decision names let them; and so are the host's own settings for approvals (whoever deployed
the version kept out, no agents, no step for whoever started the run), which no log holds.

### The state, run to run

Each run's log entry holds the state it began with, whole, and names the state it left by its
hash (a snapshot, the one `holon restore --to-run` puts back). `verify` makes each state's hash
again from the entries, and checks that a run began from the state the run before it left, and
that what its writes make of the state it began with is the state it left. So a state can't be
changed between runs, or a run's writes dropped, without the report saying where, even in a log
chained again after the change.

The log's first run began from nothing: an empty state, after no commit. In an export from the
log's first line, that is checked: it says `run 1 began from nothing, as a log's first does`.

What it doesn't check, it says, by why, and the export is then INCOMPLETE (exit 3), as one with a
run not run again is:

- a run with no run before it here, in an export that begins after the log's first run (a log
  trimmed to its newest runs, one run's export): what it began from, the runs before it would
  check. In a trimmed log, so are those that ran beside it and began after a commit older than
  every one here, whose run's line is gone. Export from the first run to have it checked;
- in an export from the log's first line, a run with no run before it whose line says it began
  from a state, or after a commit, as no log's first run does;
- a run after a deploy, a stop, or a restore of a version's state, which put a state in place
  (the events say which). After a restore of a run's state, the next run is checked: it began
  from the state that run left;
- a run whose state holds a secret's or a token's placeholder, whose hash would be the
  placeholder's;
- a run after one whose line the server couldn't write, which may have begun from the state that
  run left;
- the first run after the server started again from a crash or a power loss, with a state the
  last run here didn't leave: a run the crash cut off between its commit and its line may have
  left it. The deployment's events say so, naming the state the server loaded, and only a run
  that began from that state is taken so. After a stop (ctrl+c, SIGTERM) nothing is cut off,
  and a state the last run didn't leave is a break: it was changed while the server was stopped;
- a run after one that named no state it left: its version held none, or its state, unchanged,
  couldn't be kept.

A collection is compared by its root, not its items: a run that wrote one is checked in the state
a later run began from, its other cells as it left them. Runs of one deployment run side by side,
each committing only if what it could read is as it found it: a run that ran beside another may
have begun from a state an earlier run left, or the other began with, and only the cells it wrote
are checked in what it left. The report counts those.

A run's line is numbered once it has committed, so two runs that commit close together can be
numbered the other way round. Each entry also says which commit to the state its run began after,
and its own (`commit`): the run before a run is the one whose commit it began after, wherever its
line is, and a run no other commit landed beside is checked whole, however close its times. A log
written before entries said so is taken by each line's place and times.

A node fed a secret, a resume token or a tainted value (a caller's API key:
[tainted keys](../language/reference.md#imports-and-settings)) gets a placeholder where the value
was: the log never holds it. A node that only moves the value (into a text, a record, a list) is
computed with the placeholder, and so is a `secure_equals` of it, since a caller's token is the
secret exactly when the log shows it as the secret's mark; what the log recorded for such a node
is held to what it gives, and a record that says other differs. A node that computes with what
the value holds (a signature, a hash, an encoding) can't be computed again, nor can a check of a
token that isn't the secret, of which the log keeps only that it came. What the log kept for that
node stands in, as in the editor's replay, and the rest of the run is checked against it: the
answer built from a signature, the effects, the status. The report says which runs were checked
that way, and names each such step: its leaf, the secret or token it was fed (a secret by its
name, version and fingerprint, as the run's entry has them), how many nodes and in which runs,
`hmac_sha256 fed the secret stripe_key v3 #a1b2c3d4: 3 nodes in 3 runs: 4, 17, 30, as recorded`.
What is taken from the log is still held to itself: a leaf gives the same for the same inputs,
so two runs that record other for one leaf fed the same message under the same secret can't both
be the leaf's, and the later one differs (`what the log recorded for hmac_sha256, taken from it:
run 4 recorded other for the same inputs`); and a recorded signature that isn't 32 bytes is no
HMAC's, whatever the secret (`the shape of what the log recorded for a step taken from it`).
The line that counts the runs that matched says how many matched given such a step. What such a
node produced is taken from the log, not checked: a run whose signature check the log says passed
is checked as if it had, and the named line is where the report says so.

A run's id is made from what it received. When that held a secret or a token (a bearer token
equal to a secret, a resume link's token in its path), the id can't be made again either: the log
keeps the run id it has, and the report lists those runs. That is taken from the log too, but
nothing is lost: the id binds nothing a forger couldn't compute, and the chain and the head are
what catch an entry that was changed.

Only placeholders the server wrote are taken that way. A caller who sends text that looks like
one (`‹token:paid›` in a body) gets it logged escaped (`‹‹token:paid›`, with `"escaped": true` on
the entry), and `verify` reads it back as the text it was and computes with it like any other:
the run id and every node are checked. A channel's subscribers get it as it was sent. Entries
logged before this was escaped read every look-alike as a placeholder, as they did then.

The log keeps such a node's outputs only when they can't give the secret away. A one-way leaf
(`sha256`, `hmac_sha256`, `secure_equals`) fed the secret whole gives nothing of it away, as it
is or spelled another way (a key kept as base64 and signed with as bytes); fed a part of it, what
it gives says the part, and is computed from the secret like any other value below. A hash or a
signature kept in the log is still something made with the secret: where the flow sends one, or
which route answers with one, is in its approval ("…; a hash or signature made with it goes to
its callers"). A value that holds the secret as it is gets the placeholder. A value computed from the secret (base64 of it for a Basic
auth header, a trimmed or encoded copy) no longer holds it literally, so the log gives it a mark
of its own, as it does a secret: `‹derived #7c1e9a02›`, written wherever the run had the value
(the step that made it, a header it was sent in, what the far side echoed back, the answer). The
run's entry lists each such value by a whole hash (`derived`), of which the mark is the first
eight digits: an HMAC keyed by the salts of the secrets the run was given and bound to the run's
id. One value has one mark throughout a run and another in the next run; without the secrets'
salts the hash says nothing of the value. `verify` takes the step that made the value as
recorded and names it (`base64 fed the secret api_password v1 #…`), computes every later step
that only moves the value on its mark, and compares what the run sent and answered, marks and
all. It also checks that every such mark in a line is the start of a hash the entry lists.

A mark stands for a text of 6 characters or more. A true or false computed from a secret
(`key.startsWith(p)`, `key === v`), a number (`key.length`), bytes, a list, a field of a JSON
secret, a shorter text, and what a secret chose (`rec[key]`, `key ? a : b`) can't be marked, and
nor can what is computed inside a function applied to each item, inside a module, or deeper than
the log records a run's steps. For a run with any of those the log keeps no outputs for the
nodes that may compute from a secret; an outbound request built from one is logged with those
parts reading `‹withheld: computed from a secret›`, and so are the run's own outputs, its
response body and any other effect built from one. `verify` doesn't run such a run again, and
says which step and which secret: "`text_contains` computed a true or false from the secret
api_password: only a text of 6 characters or more is carried through a replay". To keep a run
checked, keep facts like that out of the secret (a setting of its own beside it), and compare a
secret whole with `secure_equals`.

A run's steps show which way a choice went, a choice by a secret included:
`key.startsWith("live_") ? a : b` lists the steps of the branch it took. That is by design:
whoever approves a flow reads what it chooses by, and reading a run's steps is a grant of its
own. What a secret chose to ask by is kept out of the log of a run like that. A read by one (a
collection's item under a key computed from it, a file it names) keeps which collection, folder or database it read and withholds the rest of what
it asked and what it found, but a database's rows. A header whose name is computed from one
withholds the request's headers, names and all. And `sort` and `unique` of a list that holds a
secret give something computed from it: where it came in the order. In any run, a key a secret
chose reads `seen[‹withheld: computed from a secret›]` where the log lists what the run claimed,
and in the 503 a run kept waiting on it answers; a read that failed by one says only that the
read failed.

A value computed from a secret is never kept, marked or not: a state, collection, database,
channel or file write of it is refused, as a write of the secret itself is.
A part of a file a run read that holds a secret isn't kept in the log, and a file text that holds
one isn't shipped in the export: verify asks for the file, as it does for a folder that keeps
hashes. Given it, such a run differs, since the log holds the secret's placeholder where the file
holds its value.

A credential a call answers with is kept out the same way when the request held a secret (an
OAuth token exchange): a value under `access_token`, `refresh_token`, `id_token`, `session_token`,
`token`, `client_secret`, `api_key`, `secret` or `password` in a JSON answer reads
`‹token:answer access_token #a1b2c3d4›` wherever it appears in the run, with a fingerprint like a
secret's. The run still sends it on, and checks, since the log's request holds the same mark. A
write of it to state, a collection, a database, a channel or a file is refused, so a flow can't
cache one; a run that tried is counted as one that wrote a secret's value. The same answer to a
request that held no secret is an ordinary value. Only those field names are recognised.

Some runs can't be run again at all, and are counted, not failed; an export with any exits 3: a
run whose such node the log keeps no outputs for (it computed from a secret something no mark
stands for, or the node's outputs are larger than 16 KiB or deeper than the log's timeline
records), a run that panicked (its record says what it did by then
isn't in it, and must say it did nothing), a run that failed and, run again, gets further than its
entry says (a budget ran out, a claim waited too long, or the server dropped what it did past a
limit of its own), a run
that read a file whose text the export doesn't hold, and a run that wrote a secret's value, or
one computed from it (which the server refused). A failed run's entry may say less than the run does again, never other: the
same run id, each output it keeps as computed, what it did and sent the start of what it does
again, and the status it answered either that or a failure's (a 5xx, or 409 for an unmet
`expect:`). One that says other differs, as a run that didn't fail would. A request no route took, and a run turned away before it began (a resource
an admin set to `run` was held), ran nothing: their entries are checked to say they did nothing,
and neither counts.

### Files

A run that read a file needs the file's text to run again. The export ships the texts the server
keeps, unless it was told not to. For runs whose texts aren't in the export (a folder bound with
`--keep hash`, or an export made with `--without-files`), give the folder of the original files:

```sh
holon verify inbox-report-runs.json --files /srv/drop
```

Each file is found by its hash, wherever it is under that folder.

## Witnesses

The chain shows a run changed after the fact, unless whoever changed it also rewrote every entry
after it. Someone who holds the server's data can do that. A **witness** stops it: another server
that is sent each new head as it is made, and keeps them. A head the witness holds can't be
rewritten.

Start the server with a witness to send heads to:

```sh
HOLON_EXPORT_HEADS_TOKEN=… holon serve --export-heads https://witness.example.com/heads
```

Every five seconds it sends the new heads (`{batch, heads: [{deployment, kind, seq, head, at_ms}]}`,
`kind` `run`, or `event` for the events' chain) with `Authorization: Bearer` the token. Only hashes leave the server: nothing a run read or
answered. If the witness can't be reached, the heads wait in a queue and are sent later. Past
10,000 waiting, each deployment's newest head stands for the ones before it: it pins every run
before it, so the witness is given fewer heads, and loses no run.

A witness can be any service that keeps what it is sent. The **witness** sample is one, written as
a flow: deploy it on another machine, set its `witness_key` secret to the same token, and point
`--export-heads` at its `/heads` route.

To check an export against a witnessed head:

```sh
holon verify notes-api-runs.json --head 3f9c2a7be01d…
```

```
  witnessed head 3f9c2a7be01d: run 42, so no run from 0 to 42 was changed or removed
```

The head is the hash of one run's line, which chains back through every run before it, but only
the runs in the export can be checked against it: the line names them, from the oldest the export
holds. An export of a log trimmed to its newest runs (the newest 500 by default, `--keep-runs`)
says `from 500 to 742`, and adds that older runs were trimmed: it says nothing of them. To check
them too, export before they are trimmed, or keep more.

An event's head is checked the same way: `witnessed head …: event 7, so no event up to it was
changed or removed`.

An export `holon export` writes is plain: whoever holds the file reads every run in it, and you
decide how to keep it. The ones the server keeps on disk itself, a backup's archives and a removed
deployment's records, are sealed under its keys, as everything in its data folder is. `holon
verify` opens one with the server's root key; on another machine, give it the data folder's
`keys.json` and where the root key is:

```sh
holon verify holon-archive-notes-api-runs-…json.gz --data /srv/holon/data --root-key file:/etc/holon/root.key
```

A run the server couldn't write is said lost in the next line, whose head the witness holds.
Someone who removes a run and rewrites the lines after it to say it was lost makes a log that
checks without a witnessed head, as a full disk's does; with one, it fails, the head no longer
there.

Give the whole head, as the witness holds it.
