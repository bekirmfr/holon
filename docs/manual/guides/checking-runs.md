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
trimmed, and the chain is checked from the oldest kept run.

## Exporting

From the editor: **export runs** above the run list saves `<slug>-runs.json`.

From the command line, on the machine where the server runs:

```sh
holon export notes-api
```

```
wrote /home/you/notes-api-runs.json: 42 runs of "notes-api", head 3f9c2a7be01d
```

The file holds the run log exactly as written and every unit the runs ran on, so it needs
nothing else to be checked. Options:

| Option | |
|---|---|
| `--to FOLDER` | where to write it (default: here) |
| `--as NAME` | its name (default `<slug>-runs.json`) |
| `-o PATH` | the whole path, in place of `--to` and `--as` |
| `--server PORT\|URL` | which server: with several running here, a port; a server elsewhere, its URL, with an API token in `HOLON_TOKEN` |
| `--max-files-bytes N` | ship at most `N` (`64K`, `10M`, `2G`) of the file texts runs read, the oldest runs' first |
| `--without-files` | ship none of them |

## Verifying

On any machine with `holon`, with no server and no network:

```sh
holon verify notes-api-runs.json
```

```
notes-api: 42 runs, runs 1 to 42
  chain: unbroken
  head: 3f9c2a7be01d is the newest run's
  re-ran 42 runs: each produced what its log entry says
  3 runs: 4, 17, 30 checked given what the log kept for 3 nodes fed a secret or a resume token (not computed again)
  2 runs: 9, 31 checked given the run id the log kept (made from a secret or a resume token the run received; not computed again)
ok
```

`verify` checks that:

1. the chain is unbroken, and the head is the newest run's;
2. every unit in the export is what its hash says;
3. every run, run again from its log entry through the same core, produced what the log says:
   the same outputs, the same effects, the same status. Outbound calls are answered from the log,
   so nothing is sent.

It exits 0 when everything checks, and 1 otherwise, with what failed:

```
  chain: BROKEN at run 17: changed, removed or inserted outside holon
  run 23 DIFFERS: …
FAILED
```

A node that computed with a secret or a resume token (a signature, a token check) can't be
computed again: the log never holds their values, only a placeholder. What the log kept for that
node stands in, as in the editor's replay, and the rest of the run is checked against it: the
answer built from a signature, the effects, the status. The report says which runs were checked
that way, and for how many nodes. What such a node produced is taken from the log, not checked:
a run whose signature check the log says passed is checked as if it had.

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
(`sha256`, `hmac_sha256`, `secure_equals`) or a Boolean answer gives nothing of it away. A value
that holds the secret as it is gets the placeholder. But a value computed from the secret (base64
of it for a Basic auth header, a field of a JSON secret, a slice) no longer holds it literally,
so the log keeps no outputs for that node. An outbound request built from one is logged with
those parts reading `‹withheld: computed from a secret›`, and so are the run's own outputs, its
response body and any other effect built from one. Such a value is never kept: a state,
collection or channel write of it is refused, as a write of the secret itself is.

Some runs can't be run again at all, and are counted, not failed: a run whose such node the log
keeps no outputs for (computed from a secret, larger than 16 KiB, or deeper than the log's
timeline records), a run that
failed (a budget ran out), a request no route took, a run logged before entries kept every
attempt's requests that ran again after what it read changed, and a run that read a file whose text the export doesn't hold.

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

Every five seconds it sends the new heads (`{batch, heads: [{deployment, kind, seq, head, at_ms}]}`)
with `Authorization: Bearer` the token. Only hashes leave the server: nothing a run read or
answered. If the witness can't be reached, the heads wait in a queue and are sent later.

A witness can be any service that keeps what it is sent. The **witness** sample is one, written as
a flow: deploy it on another machine, set its `witness_key` secret to the same token, and point
`--export-heads` at its `/heads` route.

To check an export against a witnessed head:

```sh
holon verify notes-api-runs.json --head 3f9c2a7be01d…
```

```
  witnessed head 3f9c2a7be01d: run 42, so no run up to it was changed or removed
```

Give the whole head, as the witness holds it.
