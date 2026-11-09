# Run records and `holon verify`

Every run a deployment makes leaves a record a third party can check: a hash-chained log entry
holding everything the run depended on and everything it produced, a head a witness outside the
server can hold, and an export `holon verify` runs again, offline, to what the log says. How the
records are synced and cut back is told in [Resilience](../themes/resilience.md) (G1, G5, G7b) and
[Performance](../themes/performance.md#2-how-a-request-goes-through); this doc is the record
itself.

## 1. What it is

- **The run log** (`crates/holon/src/recorder.rs`): one `LogEntry` a run, in memory and in
  `<deployments>.logs/<slug>.jsonl` (owner-only), older runs in sealed segments beside it.
- **The witness** (`heads.rs`): with `--export-heads URL`, each head goes to a server outside.
- **Exports** (`verify.rs`): `GET /deployments/<slug>/export` and `holon export`; the backup writes
  the same, gzipped ([Storage](storage.md#the-backup)).
- **`holon verify`** (`verify.rs`, `run`, `check`): checks an export and runs every run again.
- **A replay** (`verify/replay.rs`): `GET /deployments/<slug>/replay?unit=<hash>` runs a new
  version, offline, on a deployment's last runs, and says which it answers the same.

The sample `witness` is a flow that takes the heads a server sends (`holon serve --export-heads`),
keeps each batch and answers a receipt; `rolling-log` shows retention.

## 2. How it works

### An entry

A `LogEntry` holds a run's whole input and output: `seq` (one more per entry, across sessions),
`prev`, `session`, `at_ms`, `kind` (`request`, `trigger`, `tick`, `file`, `resume`), `route` (the
handler's label), the `request`, the `sourced` values (`Clock`, `Random`, `NetworkIngress`),
`state_before`, `state_after` (a snapshot's hash in the content store), `outputs`, `performed`,
`response_body` (cut at 64 KiB), `error`, the `timeline` of each step, the outbound `rounds`, the
`run_id` and its `nonce`, the `unit` (version) it ran on, the `grant` (hash) it was held to, the
collection `reads`, `claims`, `retries`, `by` (who sent it over the control plane), the `secrets`
it was given by version and fingerprint, and for a durable run's part `parked`, `progress` and
`events` ([Durable runs](durable-runs.md)). Evaluation is pure, so this is enough to run it again
exactly. An entry is written as the control plane serves it: secrets and tokens already redacted
(`‹secret:name›`, `‹token:name›`), a caller's look-alike escaped (`escaped`).

### The chain

Each entry's `prev` is the BLAKE3 hash of the line before it, exactly as written (`line_hash`);
the log's `head` is the hash of its newest line. The line is made before the log's lock
(`Made::line`, which splices in `seq` and `prev` under it). Opening a file checks the chain: an
entry whose `prev` isn't the line before it (changed, removed or inserted outside Holon) is a
**break**, recorded by its seq (`Log::breaks`), printed when the server loads the deployment
(persist.rs) and answered by `GET /deployments/<slug>/log` beside `head`. Entries from before the
chain (no `prev`) check only once one with a `prev` has been seen. A line half-written by a crash
is skipped (`Log::cut`).

### Keeping and cutting back

A log keeps the newest `keep_runs` runs (default `DEFAULT_KEEP`, 500, or the host's
`--keep-runs`), none older than `keep_run_days` when set: the flow's `keep` setting
(`flow("rolling-log", { keep: { runs: 5, days: 1 } })`) or the deploy settings, approved past the
host's default, up to 20,000 runs and 3,650 days (`LogLimits::ceilings`). Cutting back keeps the
newest lines byte for byte, so the chain still checks from the oldest kept, whose `prev` names one
no longer there; the newest line is always kept, carrying the numbering and the chain on. A
cut-back is made outside the log's lock
([G5](../themes/resilience.md#g5-no-run-waits-while-its-log-is-cut-back)); what it lets go of goes
to the backup first when there is one.

**Sealed segments.** A log keeps its newest `HOT` (128) runs in its file as written and seals the
older ones it keeps into `<slug>.sealed/<first>-<last>.jsonl.gz` as it cuts back (`split`,
`write_segment`); each segment records the `prev` its first line names, so the chain runs across
segments unchanged. A cut that crashes between sealing and replacing the file leaves runs in both;
opening reads each once.

**In memory.** A log on file keeps its newest `IN_MEMORY` (32) runs whole and of the rest only
their seqs and times (`Log::kept`); older runs are planned under the lock (`Log::plan`) and read
from the file and segments with it released (`Wanted::read`)
([G7b](../themes/resilience.md#g7b-a-logs-memory-is-bounded), R8).

**Synced** as [G1](../themes/resilience.md#g1-an-answered-strict-run-is-on-disk) says, at the
handler's level ([Performance](../themes/performance.md#three-levels-of-record)).

### The witness

With `--export-heads URL` (or `HOLON_EXPORT_HEADS`; https, or http to this machine), each new head
is queued (`Head`) and the queue goes in one `POST` every few seconds, at most `MAX_BATCH` (1,000)
heads, named by their hash (`batch`) so a retry is harmless. `kind` is `run`, `event` (the deployment's events have their own chain)
or `trim`, sent when a cut-back drops runs (its `seq` the oldest kept, its `head` the hash of the
last dropped): runs before it are gone by the host's choice. Only heads leave the server. Before a
batch goes, the server syncs its logs (`Exporter::before_send`, `sync_logs`), so a witness never
holds a head the disk lacks. Until a 2xx, the heads wait in `<deployments>.heads/pending.jsonl`;
the witness's answer is kept as a line of `receipts.jsonl`; a failing witness is tried again after
a growing pause, up to `MAX_BACKOFF` (five minutes). `HOLON_EXPORT_HEADS_TOKEN` is sent as a bearer
token.

### Exports

`handle_get_export` takes the log's lines and head under one lock and calls `export_of`, which the
backup shares: an `Export` (format `holon-runs/1`, `FORMAT`) of the lines exactly as written,
the head, every unit the runs ran as canonical bytes, the texts of files they read that their log
keeps by hash (`blobs`, up to `?max_files_bytes=N`, none with `?without_files=1`; none holding one
of the deployment's secrets), what was left out or no longer kept, and the deployment's events with
their head. `holon export <slug>` writes it (`--to`, `--as`, `-o`, `--max-files-bytes`,
`--without-files`).

### `holon verify`

`read_export` reads a `.json` or, by its gzip magic, a `.json.gz` (a backup's). `check` then:

1. **The chain**, as the server checks it: bad lines, breaks, whether the oldest line names one
   not there (`trimmed`), and whether the last line's hash is the export's head. `--head <hex>`
   checks a head a witness holds is in the export (`witnessed`), so no run up to it was changed or
   removed.
2. **The units**, each against its hash.
3. **Every run again** (`rerun`): the same unit, request, state, clock and random, its outbound
   answers and collection reads from the entry, through the same core and rounds, compared with
   its run id, outputs, effects and status. A node that computed with a secret or a resume token
   can't be computed again; what the log kept for it stands in (`Asserting`), and the report counts
   those runs and nodes (`given`). Runs that can't be run again at all (no outputs kept for such a
   node, failed on a budget, no route took them, a file read whose text neither the export nor
   `--files DIR` holds) are counted, not failed.
4. **The states** (`check_states`): each run began from the state the run before it left, and left
   what its writes make of the state it began with.
5. **Where the secrets went** (`check_sends`): each request carrying a secret's mark went to a host
   its unit sends that secret to; **the events** (`check_events`), their chain and head.

It exits 0 when it all checks (`Report::ok`), 1 when it doesn't, 2 when it can't read the export.

### A replay of another version

`replay` runs a version that isn't the run's own on the last runs of a deployment (20 unless
`runs` says, at most 200), as `rerun` does but through the version's handler for the entry's
route, clock, file or action, and compares status, outputs, effects, the packs sent as it waits and
where it waits. A new version moves nodes, so nothing is matched by path: a request is answered by
the run's answer to the same request (`ByRequest`), a read by the run's read of the same request
(`ByRead`). What the run never asked can't be answered offline: the run is `cant_tell`, never
sent. A pure leaf fed a placeholder is computed twice, with two placeholders (`Fed`): if what it
gives depends on which, it computed with the secret's value, and the run is `cant_tell`; if not,
it passed it along or picked past it, and stands. The run id isn't compared: another version's
differs by design. The file texts come from the export machinery (`export_of`), so a file holding
a secret isn't read back, as in an export. The editor shows it on a version's review and in the
deploy dialog (`ReplayRuns.tsx`); the sample `price-rules` is the flow to try it on.

## 3. Why it is this way

- **Answered means recorded**, a record a third party can check: most of a request's cost
  ([Performance](../themes/performance.md#1-what-it-is)); strict by default (Q6).
- **A chain over the bytes as written.** Hashing each line, not a re-serialised entry, lets a
  cut-back and sealing copy lines without parsing them (C2 c, `7c08198`) and keeps the chain whole
  across them.
- **A witness outside, heads only.** Without a copy elsewhere, whoever holds the disk can rewrite
  the log and re-chain it: `exported_runs_verify_and_a_rewritten_log_doesnt` shows a re-chained copy
  still links. A head held outside fixes the past; heads carry no run data. Logs are synced before
  each send (C2 d, `e32aecc`).
- **The record keeps each step's output** (Q16), though dropping it was faster
  ([Performance](../themes/performance.md#tried-and-left)).
- **Retention per log, not per handler** (RP C5): per-handler retention would cut lines from the
  middle of the chain.
- **Only the newest 32 in memory** (R8): a log kept every run whole, 1.1 to 3.6 times its lines,
  about 86 MB for the soaks' `notes` listing and 3.6 GB at the ceiling for a 50 KB line; after, a
  server on the soaks' data started at 39 MB instead of 320
  ([RA §2.12](../../L4s/resilience-audit-L4.md)).
- **Sealed, gzipped segments, the newest 128 as written**: the soaks' logs went from 192 MB to 37
  MB, a run's append and sync unchanged ([storage L4 step 9](../../L4s/storage-L4.md)).
- **An export proves itself**: lines as written, units as bytes, so `holon verify` needs nothing
  of the server, and a backup is an export, gzipped ([storage L4 step 6, 8](../../L4s/storage-L4.md)).
- **Verify trusts only marks the log wrote**: a caller's look-alike is escaped, so verify computes
  with it instead of taking the log's word (CHANGELOG rc.27).

## 4. Reach and limits

- **Retention cuts history.** A run cut without a backup is gone; the witness hears a `trim`, so
  the gap is said, not hidden. A log's file and segments may hold up to about twice what a cut
  leaves until the syncer cuts it.
- **A changed log is detected, not repaired.** A break is reported at the entry after the change;
  without a witness, a log rewritten and re-chained whole is not detected by the log itself.
- **The witness queue's `pending.jsonl` is not synced** (Q7); the export costs 4–13% of
  throughput ([Performance](../themes/performance.md#limits)). A witness's receipt is whatever
  JSON it answers: the sample `witness` keeps batches but signs nothing.
- **A page of old runs is parsed from the file each time**: a page of 200 runs past the 128 as
  written took 492–931 ms before sealing and 528–792 ms after, on the soaks' `notes-api` (storage
  L4 step 9). A log in memory only (`--deployments :memory:`) keeps every run.
- **Some runs can't be re-run**: those computed from a secret are checked only around the node
  the log stands in for; budget failures and file reads without their text are counted, not
  checked.
- **An unrecorded run leaves no entry** ([G8](../themes/resilience.md#g8-an-unrecorded-run-leaves-no-entry-failed-or-not-and-is-counted)),
  so verify can say nothing of it; a run that panics leaves none either (G3).
- **Exports are whole**: one JSON value made in memory; nothing streams or exports incrementally.

## 5. What the tests hold

In `recorder.rs`: the chain (`each_entry_names_the_line_before_it_and_the_chain_survives_cutting_back`,
`a_line_changed_removed_or_inserted_outside_holon_is_a_break_at_the_entry_after_it`,
`entries_from_before_the_chain_read_back_and_the_chain_starts_after_them`); keeping
(`a_kept_log_reads_back_after_a_restart_skips_a_cut_line_and_stays_bounded`,
`a_log_keeps_runs_by_age_and_reads_back_more_when_it_keeps_more`,
`keeping_more_again_forgets_nothing_the_log_was_doing`: a break, a line to sync and the backup kept
when the keep is raised again); sealing (`a_log_seals_its_older_runs_and_reads_them_back_as_it_wrote_them`,
`a_run_sealed_and_still_in_the_file_after_a_crash_is_read_once`); memory
(`a_log_on_file_keeps_its_newest_runs_in_memory_and_reads_the_rest_from_its_file`); and the
cut-back and sync tests in [Testing](../themes/testing.md#log-cut-backs-and-sweeps).

In `heads.rs`: `heads_wait_on_disk_until_the_witness_takes_them_and_its_receipt_is_kept`,
`a_batch_goes_only_after_its_hook_has_run` (the logs synced first), and three on the queue and the
URL.

In `tests.rs`, through the routes: `a_flow_log_is_a_chain_and_a_changed_run_shows_as_a_break`;
`exported_runs_verify_and_a_rewritten_log_doesnt` (every run re-runs to its entry; a copy
re-chained after a change links but that run no longer re-runs; a witnessed head is found only in
its own copy); `verify_checks_that_each_runs_state_follows_from_the_one_before`;
`a_run_that_received_a_secret_or_a_token_keeps_its_logged_run_id`;
`an_export_goes_to_a_folder_as_a_name`; `a_deployment_keeps_as_many_runs_as_its_retention_says`;
`the_runs_a_log_lets_go_of_are_backed_up_as_exports_verify_checks` (a backup's `.json.gz` verifies);
the handler tests that end "…and verify" (file handlers, actions, clocks, folders); and the golden
record `the_server_records_what_its_golden_record_holds`, which holds the entries and hashes of
every sample that needs nothing outside the server.

A replay: `a_version_replays_on_its_deployments_recent_runs` (same, differs, can't tell, no
such action, nothing changed), `a_replay_answers_a_request_the_run_sent_and_cant_tell_one_it_didnt`,
`a_replay_cant_tell_a_run_that_computes_with_a_secret` (and is told one that passes it along).

End to end: `a deployment's runs export from view mode and from holon export, and holon verify
runs each again`, and the `inbox-report`, `log-archive`, `journal`, `drop-box` and `file-audit`
tests, each exporting and verifying its runs.

**Gaps.** No test sends heads to the `witness` sample end to end; nothing in CI checks the
witness's cost; the directory sync after a cut-back's swap is untested
([Testing](../themes/testing.md#4-gaps)).

## 6. History

The chain, the witness and `holon verify` predate the CHANGELOG's first entry (rc.15).

| Commit or release | Change |
|---|---|
| rc.16 | `holon verify` checks runs that made requests (it answered only a durable run's earlier parts) |
| rc.27 | Verify trusts only the marks the log wrote; a run that received a secret or token keeps its logged id |
| `18aee8e` | `holon verify` checks that each run's state follows from the one before (rc.30) |
| `e6c34b2`, `a598533` | Retention a deploy setting (`keep_runs`, `keep_run_days`), then a line in the flow's text |
| `8c28340`, `7c08198` | A lock per log through an open file (C2 a, b); cut-backs copy lines by byte range (C2 c) |
| `e32aecc`, `4191326` | Logs synced every second and before each witness send (C2 d); the witness's queue file kept open |
| `b1259e3` | Strict by default, lazy where a handler says (C2 e, C5) |
| `0e482b4` | The line made before the log's lock (12.4 c) |
| `dc21f76` | Cut-backs outside the lock (R4) |
| `824db82` | A log keeps only its newest runs in memory (R8) |
| `4e3d47f` | What a log lets go of backed up as gzipped exports; `read_export` reads `.json.gz` |
| `09f0bdc` | Logs seal their older runs, gzipped |
| `ca69399`, `d896a4c` | Review: raising the keep forgets nothing; a failed swap leaves no segment |
