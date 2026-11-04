# Storage

How the server keeps what it stores bounded and seen: the sweeps, the packs, the logs' sealed
segments, the backup, the warnings, the host view, and what happens when a disk fills. The design,
and a "Done" note per step with its measurements, is the [storage L4](../../L4s/storage-L4.md)
(SL4); the guarantees it serves are [Resilience](../themes/resilience.md) G6, G7b and G9; the
user-facing text is the [server manual](../../manual/reference/server.md#what-it-holds).

## 1. What it is

A server keeps each deployment's log, the states its runs left (values in the content store), the
texts of files its runs read, and, in memory, its caches and live states. Retention cuts runs from
a log as new ones come; what only those runs named is then garbage on disk. This capability is the
housekeeping that removes it, the view of how much is held and where, the policy an admin sets,
and what the server does when a disk is nearly full.

In the code: `crates/holon/src/storage.rs` (the housekeeping thread, the host view, the warnings,
`when_full`, the backup's exports, `GET /admin/host`, `GET /admin/warnings`, `POST /admin/storage`,
`holon host`); `sweep.rs` (the content store's sweep, the server's and an admin's);
`crates/holon-host/src/packs.rs` (long-lived values, packed); `recorder.rs` (sealed segments, and
what a cut-back writes to the backup); `folders.rs` and `blobs.rs` (the kept file texts' sweep);
`permissions.rs` (the `[storage]` policy: `Storage`, `Threshold`, `WhenFull`).

## 2. How it works

### The policy: `[storage]`

In `holon.toml`, read at start and on `POST /permissions/reload`. The admin page and `holon policy`
set it through `POST /admin/storage` (`set_storage`: the key written with `toml_edit`, the file's
comments kept, the whole checked as a policy, written atomically, then reloaded). The keys
(`SETTINGS`) and their defaults (`Storage::default`):

| Key | Default | Says |
|---|---|---|
| `sweep_every` | `10m` | how often the server sweeps; `off`: only an admin's; at least a minute (`SWEEP_EVERY_LEAST`) |
| `warn_at` | `85%` | a disk full enough to warn of: a share in use (`Threshold::Full`) or space left, `5G` (`Threshold::Free`) |
| `full_at` | `95%` | full enough for `when_full`; refused if not past `warn_at` when both are in the same unit |
| `when_full` | `warn` | `warn`, `pause` or `roll` |
| `backup` | none | a folder's whole path where the runs a log lets go of are written first |

"Full" is full for the server: what isn't free to it (`f_bavail`), so a quota or the blocks kept for
root count as full (`disks`, `Threshold::past`).

### The housekeeping thread

`storage::start` spawns one thread that calls `Sweeps::tick` at once, then every `LOOK_EVERY` (10
s), so a changed setting applies within it. Each look:

1. **`watch`**: notes whether either cache let go of anything since the last look (over the last
   hour, `LOOKS_AN_HOUR`), computes `warnings`, and says each once in the server's log and its
   activity as it starts, and as it ends.
2. **`act`**: gives every log the backup the policy names now (`Log::set_backup`), then does what
   `when_full` says if a disk is past `full_at` (below).
3. **`archive`**: makes the backup's batches into exports (below).
4. **A round of sweeps, if due** a `sweep_every` after the last ended: the store's
   (`sweep_store`), then the folders' texts' (`sweep_folders`).
5. **`count`**: at the first look and after each round, counts what the store and the texts hold
   (`Census`), removes what crashes left, and notes each flow's disk against the last count
   (`Watch::grew`, for the warning's "the flow that grew most").

The store and the texts are counted only then, never on a view: a busy store holds millions of
files. The logs, caches, memory (`/proc/self/status`) and disks (one `statvfs` a device) are read
when asked.

### The sweeps

`sweep::sweep` lists the store's values (files named by their hash, and the packs' index), walks
from everything each deployment names (live state, sessions, versions, events, waiting runs, every
line its log's file and segments hold) through the values themselves, and removes what nothing
reaches. Any 64-digit hex counts as a name (`named_in`), so a reference the sweep wasn't taught
still keeps what it names. What was put in the last `SPARES_MS` (one hour) is spared. The server's
own sweep pauses `PAUSE` (100 ms) after every `PACE` (256) removals, and is skipped when nothing was
put since one that spared nothing (`Scheduled`); an admin's (`holon store sweep`) takes turns with
it (`Puts::sweeping`), unpaced. The mechanics and their costs are
[Resilience G6](../themes/resilience.md#g6-no-run-waits-for-a-sweep) and
[G9](../themes/resilience.md#g9-the-disk-stays-bounded). The folders' sweep (`folders::sweep`)
removes kept texts no kept run names, sparing the last `SWEEP_SPARES_MS` (5 minutes), and is
skipped when nothing is older (`Blobs::any_put_before`, R5).

### Packing long-lived values

The server's sweep collects the named values older than it spares (`to_pack`) and `pack` moves
them, up to `PACK_AT_MOST` (16,384) a pack. `Packs::pack` writes `values/packs/<id>.pack` (`MAGIC`,
raw-deflate blocks of about `BLOCK`, 64 KiB, then an index of 52 bytes a value) aside, syncs it and
renames it in with no lock held, then takes its index in under the lock; only then are the values'
files removed, paced as removals are. A crash leaves a value in both places, never in neither. A
read looks for the value's own file, then the in-memory index (`Packs::get`), keeping `KEPT_BYTES`
(8 MiB) of blocks inflated lately. A packed value named no more goes by `Packs::rewrite`: new packs
written with no lock held, then, under the store's lock, values put again meanwhile kept (a put of
a packed value writes nothing) and the index moved at once. A pack that can't be read as the store
opens is left as it is, its values missing, and warned of.

### Sealing a log's older runs

A log keeps its newest `HOT` (128) runs in its file as written; a cut-back seals the older ones it
keeps into `<slug>.sealed/<first>-<last>.jsonl.gz` and lets go of whole segments none of whose runs
it keeps (`split`). A file is cut once it holds more than twice `min(keep, HOT)` lines, or its
oldest is a day past the age it keeps. The chain across segments is in
[Run records](run-records-and-verify.md#keeping-and-cutting-back); the cut outside the lock,
[Resilience G5](../themes/resilience.md#g5-no-run-waits-while-its-log-is-cut-back).

### The backup

With `backup` set, a cut-back writes the lines it lets go of to
`<backup>/<slug>/holon-archive-<first>-<last>.jsonl` first (`keep_aside`, `storage::batch_name`,
each seq 20 digits; whole segments let go of, `keep_segments_aside`). If it can't, it doesn't cut:
the log keeps the runs and records why (`Log::backup_error`), unless `roll` is acting past
`full_at` (the admin chose room over history). Each look, `archive` makes each batch an export
`holon verify` checks (`archive_one`: `verify::export_of`, with the units the runs ran and the file
texts they read, redacted by the deployment's secrets), gzipped, as
`<backup>/<slug>/holon-archive-<slug>-runs-<first>-<last>.json.gz`, then removes the batch. Every
file the server writes there starts with `ARCHIVE_PREFIX`, and it exports or removes only a file
with a batch's exact shape (`is_batch`) and counts only its exports (`is_export`): the folder may be
a drive or a synced folder holding other things. A batch that can't be made an export stays, is
warned of, and holds up no other.

### When a disk fills: `Sweeps::act`

Past `full_at` on any disk:

- **`warn`**: the warning, nothing else. Once the disk fills, a strict run fails its sync and
  answers 500.
- **`pause`**: every running deployment that records runs (`records_runs`) is paused by the host,
  its channels closed and `grant.paused_full` set (persisted), with a note on its timeline.
  Unrecorded-only flows go on.
- **`roll`**: each look, the biggest log on disk (file and segments, `Log::sealed_holds`) keeping
  more than `ROLL_FLOOR` (20) runs is cut to half, never below the floor, and cut back at once
  (`LogCell::cut_if_planned`); the first cut of a full disk sweeps the store after it.

Once every disk is under both `warn_at` and `full_at`, what was paused for it is resumed and what
was rolled keeps its runs as granted again (`Watch::rolled`; what was cut stays cut). A flow a
person resumes while the disk is full is paused again at the next look.

### What the host sees

- **`GET /admin/host`** (`host_view`, admins; `holon host`, the admin page, the terminal editor):
  memory and each cache against its size; each disk once, with what it holds (`places`); what is
  kept there (`kept_view`: the store's values loose and packed and how many were put since the
  count, the logs with their segments, the texts, leftovers removed and waiting); each flow's
  holdings; the sweeps; the thresholds; what `when_full` is doing (`acting`); the backup; warnings.
- **`GET /deployments/<slug>/holds`** (`handle_get_holds`; `holon host --for SLUG`): its log in
  memory (its entries' line bytes times `ENTRY_MEMORY`, 3) and on disk, its live state, its flow in
  the cache (own and shared), and its share of the store and texts as the last store sweep walked
  them (`Sweeps::note`: a value is the first flow's to reach it, counted once as shared if another
  reaches it too).
- **`GET /admin/warnings`**, by key: `disk:<path>`, `cache:flows`, `cache:values` (at its size most
  looks of the last hour), `backup` (a log couldn't write to it), `backup-export` (a batch couldn't
  be made an export), `pack:<path>`, `sweep-off` (off, with the store past a tenth of its disk).
  Each says what to do.

**What crashes leave**, a file written aside and never renamed in (`is_aside`), is removed by
`count` once older than `LEFTOVER_AFTER` (15 minutes), and said.

## 3. Why it is this way

- **The server sweeps on its own**: with only an admin's sweep, the soak's 51 writes a second
  filled the disk by about 5.4 GB an hour (R6, [RA §2.11](../../L4s/resilience-audit-L4.md)).
- **Paced, because removing is what costs**: an unpaced sweep of 134,000 values took a `notes`
  listing from about 380 requests a second to 123; paced, it loses 7–12% (RA §2.11).
- **One thread, every 10 s**: one place asks "is something due", and a setting applies without a
  restart (SL4 step 1).
- **Counted after sweeps, not on a view**: the sweep walks the store already and attributes each
  value on the same pass (SL4 step 3); the cost is that a share is "as of" the last sweep.
- **Packs**: 7,974 values took 31.3 MiB of disk for 2.2 MiB of bytes, a block each (SL4 step 2);
  packed, a copy of the soaks' store went from 140,026 files (562 MB) to one pack of 1.0 MiB (step
  10). Written with no lock held, because under the index's lock every put and every read of a
  packed value waited for the pack's I/O (`a6babdf`).
- **Sealed segments**: on a copy of the soaks' data the logs went from 192 MB to 37 MB within 8
  seconds of starting, lazy and strict `hello` and the `notes` listing no slower (SL4 step 9). The
  newest 128 stay as written so a run's append and sync are unchanged.
- **A backup that can't be written keeps the runs**: retention was asked to move them, not lose
  them (SL4 §4). Gzipped exports, because an export proves itself offline: `hello-api`'s 10,669
  runs went from 51 MB to 3.9 MB, `notes-api`'s 10,049 from 618 MB to 20 MB (steps 6, 8).
- **Only its own files**: a file of the admin's own on the backup drive was once "exported" empty
  and deleted (`7c0b545`); the prefix and the exact shape make every file the server touches its
  own by name ([Lessons](../themes/lessons.md#what-the-review-found)).
- **`warn` by default**: nothing changes behaviour until an admin chooses (SL4 Q2). `pause` keeps
  every promise and stops answering; `roll` keeps answering and shortens history.
- **`roll` cuts one log a look and sweeps once**: sweeping after each cut held the next up for the
  length of a sweep, 73 s on the soaks' data (SL4 step 5). It cuts before it sweeps, because a cut
  left to the syncer came after the sweep, which then freed nothing (`159a6db`).
- **Undone only under both thresholds**: written in different units, `full_at` can be reached
  first; undoing under `warn_at` alone paused and resumed every flow every ten seconds (`ecc92ce`).

## 4. Reach and limits

- **Measured live** (SL4 step 5, the soaks' data, this container's disk 98% full for the server):
  `roll` cut `crunch-lazy` and `notes-api` first and kept runs as granted again once there was
  room; `pause` paused the 9 flows recording runs (a strict route answered 503, an unrecorded one
  200) and resumed them.
- **Packed reads are slower without the value cache**: the `notes` listing with none answered
  636–672 requests a second from loose files, 74–82 from a pack keeping 16 inflated blocks, 811–825
  keeping 8 MiB; with the default value cache, 2,201–2,355, as before (SL4 step 10, measured once).
- **A packed value's put time is only in memory** (`Puts::recent`): a restart forgets it, so a
  packed value put again in the hour before a restart, by a run not yet committed, isn't spared.
- **No object storage**: `backup` is a folder; `s3://` (SL4 step 7) and Holon Cloud are not built
  (a folder a cloud client syncs reaches the cloud for now).
- **Bounded, not small**: about 5.5–6.4 GB of unnamed values while the soak's writes go on (G9).
- **An admin's sweep is unpaced and doesn't pack**; it holds up runs while it removes.
- **Shares are approximate and late**: a shared value is counted once, not split; a flow's disk
  share is as of the last store sweep; its log memory is an estimate (×3; measured 1.1 to 3.6).
- **The disk is the filesystem's**: another program filling it counts too. A look every 10 s means
  `when_full` can act up to that late.
- **`pause` and `roll` act on every flow that records runs**; per-flow choices are not built (SL4
  §5). `roll` frees one log a look, never below 20 runs, and frees nothing a log didn't hold.
- **Segments go whole**: one with any run still kept stays, with the rest of its runs.
- **No backup of the whole data folder** (accounts, secrets, deployments file): that is a machine's
  backup (SL4 §5).
- **No sample shows `when_full` or the backup**: `rolling-log` shows retention, the sweep and the
  host view; SL4 asked for a flow that fills its log fast for steps 3 to 6, and none was added.

## 5. What the tests hold

In `crates/holon/src/tests.rs`, through the routes:

- `the_server_sweeps_its_store_on_its_own_and_not_again_for_nothing` (R6),
  `the_server_sweeps_as_often_as_its_storage_setting_says`,
  `an_admin_sets_how_often_the_server_sweeps_in_its_policy_file` (other sections and comments
  kept), `a_store_sweep_clears_the_states_no_kept_run_names_and_nothing_else`,
  `an_in_memory_store_has_nothing_to_sweep`.
- `the_host_says_what_it_holds_in_memory_and_on_its_disks`,
  `each_flow_says_what_it_holds_in_memory_and_on_disk`,
  `the_host_warns_of_a_full_disk_and_a_full_cache_once_each`.
- `a_full_disk_pauses_what_records_runs_until_there_is_room` (a `full_at` reached before `warn_at`
  pauses once and holds), `a_full_disk_rolls_the_log_keeping_the_most`,
  `a_full_disk_rolls_the_log_biggest_with_its_sealed_runs`,
  `a_full_disk_rolling_sweeps_what_only_the_cut_runs_named`.
- `the_values_a_sweep_keeps_are_packed_and_read_back` (and swept from the pack once named no more),
  `a_pack_that_cant_be_read_is_warned_of_and_the_store_opens`,
  `the_states_sealed_runs_left_are_kept_by_a_sweep`.
- `the_runs_a_log_lets_go_of_are_backed_up_as_exports_verify_checks`: every run once across log and
  backup; the folder's own files (one named as a batch was before the prefix) left alone and
  uncounted; a batch that can't be exported holds up no other; a backup that can't be written keeps
  the runs in the log.

Unit tests: `storage.rs`'s names and leftovers
(`a_backup_batch_and_its_export_are_known_by_their_names_and_nothing_else_is`,
`what_a_crash_left_is_removed_once_it_is_old_and_nothing_else_is`); `packs.rs`'s
`a_rewrite_hides_no_value_it_keeps_and_keeps_one_put_again` (a thread reading throughout) and
`a_pack_cut_short_is_said_and_the_others_read`; `recorder.rs`'s sealing and backup tests
(`a_log_seals_its_older_runs_and_reads_them_back_as_it_wrote_them`,
`a_backup_written_to_again_is_no_longer_an_error`, `a_cut_whose_swap_fails_leaves_no_segment_behind`).

End to end (`samples.e2e.ts`, `rolling-log keeps only its newest 5 runs…`): retention, a dry-run
sweep, the admin page's host view and each flow's row, `warn_at` set to `1000T` free and back.

**Gaps.** Nothing in CI fills a real disk (the tests pick thresholds no disk meets, or every disk
is past); nothing checks the pacing's cost or a packed read's throughput; no end-to-end test sets a
backup or `when_full`.

## 6. History

| Commit | Change |
|---|---|
| `a16e2ba` | `holon store sweep` clears the states no kept run names (an admin's sweep) |
| `42a75f9`, `6d764f5`, `de42b69` | R5, the folders' sweep holds up no run; R6, the server sweeps its store; paced |
| `f4b80ec`, `5b921ed` | The storage design (SL4); `[storage] sweep_every`, one housekeeping loop (step 1) |
| `1e7bcf1`, `4b198a0` | The host says what it holds; each flow says what it holds (steps 2, 3) |
| `824db82` | A log keeps only its newest runs in memory (R8, found by step 3) |
| `d46f986`, `156a5f0` | `warn_at` and the warnings; `full_at` and `when_full` (steps 4, 5) |
| `4e3d47f` | The runs a log lets go of backed up as gzipped exports (steps 6, 8) |
| `5b35b42`, `09f0bdc` | The store's kept values packed (step 10); logs seal their older runs (step 9) |
| `7c0b545`, `7eebc94` | Review: the backup touches only its own batches; one bad batch holds up no other |
| `ecc92ce`, `ca69399`, `f288afe` | Review: undone only under both thresholds; raising the keep forgets nothing; a backup written again is no longer an error |
| `159a6db`, `7a0e26a`, `d896a4c` | Review: `roll` cuts before it sweeps; a log's disk counts its segments; a failed swap leaves no segment |
| `91cc5e2`, `a6babdf` | Review: an unreadable pack is warned of and the store opens; packs written with no lock held |
| `9b4bd37` | Every file the server writes to the backup starts `holon-archive-` |
