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
  re-ran 39 runs: each produced what its log entry says
  not re-run (3 runs: 4, 17, 30): it used a secret or a resume token, which the log never holds
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

Some runs can't be run again, and are counted, not failed: a run that used a secret or a resume
token (the log never holds their values), a run that failed (a budget ran out), a request no route
took, a run logged before entries kept every attempt's requests that ran again after what it read
changed, and a run that read a file whose text the export doesn't hold.

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
