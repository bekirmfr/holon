# Resilience

What the server guarantees under load and failure, how each guarantee is kept, and where it
stops. The evidence is in the [resilience audit](../../L4s/resilience-audit-L4.md) (RA), which
went over the throughput work (`aa2704a..9969f8b`) looking for what it had cost, and the soaks
that followed it.

## 1. What it is

Holon's server runs many deployments side by side, each recording its runs. Being fast is worth
little if one deployment's load can stall another, if a crash loses a run that was answered, or
if the disk fills on a quiet afternoon. These are the guarantees the server keeps, stated so
they can be tested, and the work that made them true. Where a guarantee and speed pulled apart,
the guarantee won ([Standards](../README.md#standards-these-docs-record)).

## 2. The guarantees

### G1. An answered strict run is on disk

A `strict` run (the default) is answered once its log entry is synced: `log_run` calls
`LogCell::sync_through(seq)` (`firing.rs`, `recorder.rs`), and every run waiting at once shares
one sync. A line that couldn't be written, or a sync that failed, is answered with a 500 ("the
request ran, but …"), never with a success. Whether its line was written is asked whichever sync
covered it, its own or another run's: a sync says what was written is on disk, nothing of an
entry that wasn't (a run whose line was lost was once answered 200 when the run after it had
synced first; `lost`, read without the log's lock, keeps the usual case as cheap as it was).

- **Limits.** A `lazy` run is on disk within `SYNC_EVERY` (one second): a power loss can lose the
  last second of them (a stop syncs them first, G10). Requests no route took are recorded lazily (Q12). A run nobody waits for
  (a clock's, a file handler's, one resumed) only says its record may not be on disk yet. The
  log remembers the last 1,024 entries whose line was lost (`unkept`): one asked for after as many
  more were lost would be told it is kept, which no load reaches (a run asks as soon as it wrote).
- **Tests.** `a_strict_run_is_on_disk_before_its_answer_and_a_lazy_one_waits_for_the_syncer`,
  `a_run_whose_record_isnt_on_disk_is_never_answered_as_done_and_its_sends_wait` (a table: a
  request and an action, strict and lazy, every sync failing; strict is a 500, lazy a 200, no
  pack goes out until a sync puts its record on disk, then each goes out once),
  `a_run_is_told_its_record_is_kept_exactly_when_its_line_is_in_the_file` (a model: entries
  written, some writes failing, asked for in drawn orders between the syncer's ticks, some syncs
  failing, and by runs side by side; the cell syncs what the model says, a failed sync leaving
  the file to sync again, each run is told what its own write and sync did, and the file holds
  the lines told kept, and told unsynced, and no other; it replaced a test of a sync failed by
  hand), `one_sync_covers_every_entry_written_before_it` (the economy of it: one sync a batch).

### G2. A deployment can't slow another

No run computes on the data plane's tokio workers: every run goes to the blocking pool (`answer`,
`dataplane.rs`), however short, and the workers only accept connections and move bytes. Each
deployment's log has its own lock, and the line is made before taking it.

- **Why none.** Of the idle workers one watches the network, and the one that reads a request is
  that one: a run on it, even one that never waits, left every listener deaf until it ended (a
  route computing 3 s held a 413 on another port 1.65 s). Build step 12.1 answered short routes on
  the worker; R2 narrowed that to runs that can never wait (a deployment busy reading its notes on
  the worker had taken an unrelated `hello` from about 25,000 requests a second to 66–403, and to
  30 with writes), which still let them compute; B5 took it out. What it saved and the ways around
  it weighed: [performance](performance.md#nothing-computes-on-a-data-plane-worker-2026-11-13).
- **Cost.** Every run pays the pool's hop: 20–30 µs at one connection on a trivial route, about a
  quarter of its throughput there; under load, even or ahead.
- **A share each.** The pool is one for every deployment, so a deployment flooded, or whose runs
  wait on a slow upstream (25 s a run fills 512 threads at about 20 requests a second), held every
  other's requests in its queue, unanswered (audit R15). Each run now takes a place in its
  deployment's `max_concurrent_runs` (64 by default, at most 448) and in the data plane's 448
  (`Share`, `dataplane.rs`): past either a request is answered 503 with `Retry-After: 1` at once,
  never queued, and a WebSocket's message or close waits for a place on a task, holding no
  thread. The 64 threads past the 448 are for a file's reads, which take no place.
- **Callers that stop reading.** Every connection's writes must make progress within 30 s
  (`write_timeout`, `Deadlined`): past it the connection is closed. A file is sent from a task, each
  64 KiB piece read on the pool; it was sent from a pool thread that waited on its caller, so
  callers that stopped reading big files held the pool for good. A streamed reply's run still waits
  on its caller for each piece, at most the deadline: then its caller is gone, and the run reads
  the rest of its upstream (bounded by the call's own limits) and ends.
- **Clocks.** A clock's tick goes on by itself, and the deadlines of waiting runs have a thread of
  their own (scheduler.rs): a tick waiting on a call holds its own clock alone. Until R34 the loop
  waited for every tick it started, then looked at the deadlines: one tick waiting 30 s held every
  clock and every deadline on the server as long.
- **Limits.** CPU is shared: 64 connections of `crunch` slow every route ("overload", RA §2.1).
  The pool runs as many runs as are computing, up to its 448 places, the cores shared among
  them; what one run may compute is its round's budgets (B5). A run whose caller went away is not
  stopped: it holds its place until it ends, within its run budget.
- **Mailboxes.** Each mailbox's look is a thread of its own, which the watcher doesn't wait for
  (`inbox.rs`): before, the looks were joined each tick, so one mailbox's server that didn't
  answer (30 s a step), or one message long to read, held every other mailbox's look. A message
  is read in one pass, whatever it holds (core's `mail::received`), and one whose look is cut off
  twice is moved without a run, not read at every look and restart.
- **Tests.** `nothing_a_run_computes_holds_up_the_data_plane` (`crates/holon/tests/data_plane.rs`),
  `one_deployments_log_doesnt_hold_up_another`, `a_fire_in_progress_does_not_stall_the_table`,
  `no_deployments_callers_hold_more_of_the_data_plane_than_its_share`,
  `a_mailbox_holds_no_other_and_a_message_that_cuts_its_look_off_is_read_twice`,
  `a_slow_run_the_clock_loop_starts_holds_no_other_clock_or_deadline`, and core's
  `a_message_is_read_in_time_its_size_bounds_whatever_it_holds`.

### G2b. The server keeps to its memory and its open files

A round is held to its own `max_memory`; the server holds every round together, and all else it
holds, to `--max-memory` (half the machine's memory by default, or three quarters of its
container's limit, `bounds.rs`). The allocator counts each thread's bytes into one process count
too (`holon_core::memory`: an atomic add per 64 KiB a thread moves, and its last moves added as
the round ends and as the thread does). A round that finds the process past its cap fails
(`RunError::ServerMemory`, its error naming `--max-memory`, which `holon verify` reads as not run
to its end, as one stopped for time); a request arriving meanwhile is answered 503 with
`Retry-After: 1` before its body is read. A round reads the count where it reads the clock, and a
leaf that makes many values in one step (`json_parse`, `csv_parse`, `xml_parse`) asks every 4,096
values whether the round may make more (`memory::room`), so a parse stops inside its step, past
the round's budget or the server's, not once it has made 80 times its text. Connections are held
to `max_connections_total`, every listener's together, a WebSocket's while it is open (`Places`):
4,096, or what the open-file limit leaves past 256 for the server's own files (on unix the soft
limit is raised to the hard one first, as Go's runtime does). An accept that fails is tried again
after 5 ms, doubling to 1 s, and said once a run of failures (audit R16).

- **Why.** 448 runs at once might each hold 256 MiB, and a parse made 80 times its text in one
  step before a check; a WebSocket held no connection place, so a server's open sockets ran past
  its open-file limit, where every accept failed and its loop spun on a core printing each time.
- **Cost.** An add, a compare and a store more an allocation, in the thread-local the count was
  already in, and one atomic add a 64 KiB moved; a round's clock reading also reads the count.
  `crunch` ×1.012 and `hello` ×1.049 in instructions ([performance](performance.md#the-processs-memory-count-2026-11-15)).
- **Limits.** The count is the allocator's: what a module's engine maps for itself, the threads'
  stacks and the allocator's own overhead are outside it, which is why the default leaves half the
  machine. A run going on as the server passes its cap fails, whether or not it grew most. A
  request's body is still copied into its record (`data::request_record`): counted now, not
  avoided. Each listener takes its places before it accepts, so each holds one as it waits.
- **Tests.** `the_runs_together_are_held_to_the_memory_the_server_may_hold`
  (`crates/holon/tests/data_plane.rs`), `no_server_holds_more_connections_open_than_its_total`,
  `each_parser_stops_inside_the_leaf_past_the_round_or_the_process`,
  `a_thread_that_ends_leaves_the_process_count_as_its_moves_do`
  (`crates/holon-core/tests/process_memory.rs`), `the_default_bounds_are_ones_a_server_runs_within`.

### G3. A panic ends its run and nothing else

A panic is caught where every run passes, `fire_with_state` (firing.rs), whatever started the run:
it becomes a run that failed (`panicked`, its error starting `PANICKED`), logged and counted like
another, and a caller is told 500 ("the request's handler failed"). One in a request's handler
before its run is caught where the connection is answered, by the blocking pool's join error (R1;
before, the connection was dropped with no answer). Before the first, a panic in a clock's run ended
the thread that ticks every deployment's clocks and the deadlines of every waiting run, and one in a
feed's, a queue's or a mailbox's run ended the thread that reads it, for good and with nothing in
any log.

- **Locks.** A lock held where the panic was is taken again by the next run, as the panic left
  what it guards: the server's locks are its own (`sync.rs`: std's, taken back from a panic, their
  `lock` answering a `Result` that is never an error), and so are the content store's packs'
  (holon-host's `packs.rs`). Before, a std lock a panic held stayed poisoned, and every later run
  or request that took it failed the same way until the server restarted: one panic holding the
  deployments table stopped the server.
- **Limits.** A panic in a request's handler before its run leaves no log entry. A run that
  panicked says so, and that what it had sent or written by then isn't in its record: a panic
  past its first round, or in its commit, is past things done. What a panic left half changed
  under a lock stays so (no lock rolls it back). logs.rs keeps std's lock, left to the work that
  owns that file.
- **Tests.** `a_panic_ends_its_run_and_nothing_else` (every way a run starts that a test drives
  without a far side, a panic in the run and before it, the deployment beside it unharmed),
  `a_staged_unit_loads_or_is_refused_whatever_its_wiring` (an effect wired into a write, the
  wiring that once panicked a run under the state lock, is among what it refuses at deploy),
  `a_panic_holding_what_every_run_takes_stops_no_run_after_it` (the deployments table, a log, a
  state, a vault, the policy), `every_lock_in_the_server_is_one_no_panic_poisons` (a sweep of the
  source), `a_lock_a_panic_held_is_taken_again_as_the_panic_left_it`,
  `a_panic_holding_the_packs_locks_stops_no_read_after_it`.

### G4. The evaluator fails closed, and its fast path answers as its plain one

A node whose kind doesn't match its body computes nothing (every output absent), never panics
(R3). The compiled plan hands a run to the walk whenever it meets a case it wasn't built for.

- **Limit.** The hand-over is silent: right, but slower. A tripwire test fails if the compute-heavy
  samples' functions stop running down a plan (D5).
- **Tests.** `a_composite_run_down_its_plan_does_what_the_walk_does` (3,000 random graphs),
  `a_plan_out_of_order_hands_over_and_a_kind_that_isnt_its_bodys_computes_nothing`,
  `the_compute_heavy_flows_functions_all_run_down_a_plan`, the golden records.

### G5. No run waits while its log is cut back

A log's file holds up to twice what it keeps; past that it is cut back. A run that finds it full
only marks it (`cut_wanted`); the syncer cuts it (`LogCell::cut_if_planned`): the kept lines
written aside with the lock released, caught up with what runs write meanwhile until under 64 KiB
is left, then swapped in under the lock (renamed over, the directory synced).

- **Why.** Cutting a 25 MB log under its lock took 84–92 ms plus syncs: every run of that
  deployment waited 100–150 ms, about once a second at 1,100 requests a second (R4). Two simpler
  designs were measured and were worse ([Lessons](lessons.md)).
- **Limits.** Between being marked and cut, a file holds past twice its keep by what a second
  brings. Cuts are made one log after another on the one sync thread. A one-connection p99 of
  about 30 ms remains on a lazy route with a busy log, likely the sync holding the next append.
- **Tests.** `a_cut_made_outside_the_lock_leaves_the_file_one_made_holding_it_leaves` (byte for
  byte), `lines_written_while_a_cut_is_made_outside_the_lock_are_all_kept_in_order`,
  `a_cut_overtaken_by_the_file_changing_is_made_afresh_and_a_deleted_log_stays_deleted`.

### G6. No run waits for a sweep

The sweeps (of the content store, and of folders' kept file texts) read each log's file a line at
a time with its lock released (`LogCell::each_line`): a file is only ever replaced whole, so the
reader sees one file's lines, in order. The folders' sweep is skipped when the blob store holds
nothing old enough to remove, and parses only the lines naming a kept text (R5).

- **Why.** The folders' sweep read every whole log under its lock every ten minutes (a 60 MB log
  held its runs up), and parsed every entry: about 200 MB, even with no folder at all. The soak
  saw it as 45 MB memory steps.
- **Removing is what costs, so the server's sweep is paced.** Each removal holds the store's lock
  and ext4 journals each unlink: removing 134,000 values back to back (27 s) took a deployment's
  `notes` listing from about 380 requests a second to 123 while it ran; reading and checking
  everything without removing cost nothing (RA §2.11). The server's own sweep pauses 100 ms after
  every 256 values (`PACE`, `PAUSE`): the listing loses 7–12% while it runs.
- **Limits.** An admin's `holon store sweep` is not paced (asked for now, it removes as fast as it
  can, and holds up the server's runs while it does). The store's sweep holds each deployment's
  state lock while it reads the live state. Its memory stays with the process: about 110 MB more
  after the soak's first sweep.
- **Tests.** `a_log_read_a_line_at_a_time_holds_no_lock_and_sees_one_file_whole`.

### G7. The caches keep to their sizes in memory

The flow cache (`--flow-cache`, 256 MiB) and the value cache (`--value-cache`, 64 MiB) count what
an entry takes in memory (`unit_footprint`, `closure_footprint`, `Shapes::footprint`,
`footprint`), each estimate rounded up a quarter, so a cache keeps a little less than its size,
never more (S1, S5).

- **Why.** They counted encoded bytes: measured with `mallinfo2`, the value cache held 2.8 times
  its size and the flow cache 7.8 times (flows without a js node), about 2 GB at the default.
  After: 1.0 and 0.8–1.0 times.
- **Limits.** The estimates are checked against real memory only by hand
  (`measure_cache_memory`), which prints and asserts nothing.
- **Tests.** `the_flow_cache_lets_go_of_the_least_used_past_its_budget`,
  `the_store_keeps_the_values_it_read_within_a_budget`,
  `the_value_caches_size_is_a_setting_it_keeps_to`.

### G7b. A log's memory is bounded

A log on file keeps its newest 32 runs in memory (`IN_MEMORY`) and of the rest only their seqs and
times; older runs are read from its file, planned under its lock and read with it released
(`Log::plan`, `Wanted::read`). Before, it kept every run it kept, whole: about 86 MB for the soaks'
`notes` listing, 3.6 GB at the `keep_runs` ceiling for a 50 KB line (R8).

- **Limits.** A page of old runs is parsed from the file each time it is read (the same time as
  cloning them was, measured). A log in memory only (`--deployments :memory:`) keeps them all.
- **Tests.** `a_log_on_file_keeps_its_newest_runs_in_memory_and_reads_the_rest_from_its_file`,
  `a_read_planned_before_a_cut_back_reads_the_runs_it_planned`.

### G8. An unrecorded run leaves no entry, failed or not, and is counted

`count_if_unrecorded` (`firing.rs`), the one rule every path calls (D2), counts the run in its
handler's tally: runs, failures, time, the last run, and the last failure's time, status and
error, redacted and cut to 300 characters. The request is never kept. The tallies are persisted
by the syncer when they changed.

- **Why never logged.** At first a failed unrecorded run was run again and logged, so its failure
  could be seen (Q10). Reversed (RA §2.8): a handler may be declared unrecorded to keep its
  requests out of the log (redaction removes secrets, not personal data); failures can be forced
  from outside, so each one cost a second run, a line of several KB and its share of cut-backs;
  and the answer came from the second run, which may have read a newer state. To see why runs
  fail, make the handler `lazy` for a while.
- **Limit.** A restart loses at most the last second of counts.
- **Tests.** `an_unrecorded_run_leaves_no_entry_failed_or_not_and_is_counted`,
  `an_unrecorded_failure_is_kept_redacted_and_cut_short`,
  `a_policy_without_allow_unrecorded_refuses_the_deploy`.

### G9. The disk stays bounded

- **Logs**: each keeps its newest 500 runs by default (`keep` and the deploy settings say more or
  fewer), its file at most about twice that.
- **The content store**: each state write leaves its changed values and a snapshot as files; once
  retention cuts the run, nothing names them. The server sweeps them every ten minutes
  (`[storage] sweep_every`, storage.rs), sparing what was put in the last hour (`SPARES_MS`), paced (G6); the sweep is
  skipped when nothing was put since one that spared nothing (R6). An admin's `holon store sweep`
  takes turns with it.
- **Why.** Only an admin's sweep removed them: at the soak's 51 writes a second (7.7 files and
  about 30 KB of disk each), the disk filled by about 5.4 GB an hour.
- **Limits.** Bounded, not small: a value is swept 60 to 70 minutes after it was put, so at that
  rate about 5.5–6.4 GB (1.4–1.65 million files) of unnamed values is the floor while writes go
  on. Fewer, larger files per write would lower it; not done. What a deleted deployment let go
  of waits for the sweep after the next put.
- **What crashes leave.** Every file is written aside and renamed over, so a server stopped in the
  middle of a write leaves the aside file; nothing removed them (60 MB in the soaks' data folder).
  The housekeeping count removes one once it is older than any write takes (15 minutes).
- **Seen.** `holon host` and the admin page say how full each disk is and what is kept on it
  (storage-L4.md, step 2), and warn past `[storage] warn_at` with what to do (step 4).
- **When full.** Past `full_at`, an admin's choice (`when_full`, step 5): only warn (the
  default: a strict run then fails its sync and answers 500 rather than answer unrecorded),
  pause what records runs until there is room (records kept, runs refused), or cut the oldest
  runs of the biggest logs (runs answered, history shortened). Each is said on the timeline.
- **Sealed.** A log keeps its newest 128 runs as written and seals the older ones, gzipped, as it
  cuts back (storage-L4.md, step 9): the run's path, its append and its sync, unchanged; a cut
  that crashes between sealing and replacing the file leaves runs in both, read once.
- **Packed.** The values a sweep keeps, once an hour old, are packed (deflated together, a pack
  written whole before their files go): a used store's 140,026 files became one pack of 1 MiB
  (storage-L4.md, step 10; the soaks' "7.7 files a write" is gone for what is kept).
- **Backups.** With `[storage] backup`, what a log lets go of is written there before it goes,
  as gzipped exports `holon verify` checks (steps 6 and 8); a backup that can't be written to
  keeps the runs in the log, so retention never loses what it was asked to move.
- **Tests.** `the_server_sweeps_its_store_on_its_own_and_not_again_for_nothing`,
  `a_store_sweep_clears_the_states_no_kept_run_names_and_nothing_else`.

### G10. A stop finishes the runs going on

Told to stop (ctrl+c, SIGTERM, its console closing), `holon serve` starts no run, lets those
going on finish, each to its log line, syncs the logs and the witness's queue, and exits
(`stopping.rs`). It ended where it stood before: a run cut off half done, its calls made and its
line never written, and a second of `lazy` lines unsynced.

- **How.** The server's call hub says it is stopping (`calls::Hub::stop`); every trigger asks
  `Deployment::takes_runs` before it starts a run, as it asks whether its deployment is paused,
  and requests are answered 503 (`admitted`, `serve_flow`). A run holds a door open from
  `fire_with_state` until its `FiredRun` drops, after its line; so do a flow's request, whole,
  and the writing of a line, so the door is never left unheld between a run and its line. It
  closes in the step that finds nothing holding it (`Door::close_if_empty`, one atomic word): a
  run a trigger let through just before, arriving after, waits there for the exit and never
  starts. A run inside a held one (a flow's call) finds the door held, so it is never shut out.
- **Why a door.** Without it, a run that asked before the stop and started after the drain saw
  none would be cut off by the exit. Refusing it in `fire_with_state` instead would fail it, and
  a queue deletes a failed run's message.
- **Limits.** A stop waits as long as the slowest run going on may take (its budget). A second
  ctrl+c exits at once, as a power loss. A message a socket sends while it stops isn't run; a
  waiting run's deadline that passes while it stops is delivered once it is back.
- **Tests.** `a_server_told_to_stop_starts_no_run_and_finishes_the_one_going_on` (every way a run
  starts that a test drives without a far side, before and after the stop, and runs with their
  calls out as it is told, an action's and a clock's tick on its own thread, which no pass of
  the clock waits for: each goes on to its line, counted at the door; and a sweep holding every
  file that starts runs to asking),
  `the_door_closes_only_on_no_runs_and_a_run_is_counted_or_turned_away` (runs racing the drain),
  `the_drain_waits_for_the_last_run_saying_how_many_are_left`.

### G11. A run a crash cut off reads as cut off, not as tampering

A run lands its state, and the head naming it, before its line is written (C3 b), so a crash or
a power loss between the two leaves the state ahead of the log. `holon verify` then found the
next run beginning from a state the run before it didn't leave: a break, as if the state had been
changed outside holon.

- **How.** A stop, once every line is on disk, leaves a mark beside the deployments file
  (`stopping::mark_stopped`), which the next start takes away (`stopped_cleanly`). A start that
  finds none follows a crash: each deployment whose state, as its head names it, isn't the one
  the last run in its log left notes a `lifecycle` event naming the state it loaded (`loaded`,
  `persist::note_cut_off_runs`). `verify` takes the run after it, if it began from that very
  state, as unchecked where it began (`verify::CUT_OFF`), not as a break.
- **Why the mark.** Said after every start, a state changed while the server was stopped would
  pass as a run cut off; with it, that still breaks after a stop. Why not a line before each
  commit, which would name the run cut off: it costs a write and a sync every strict run.
- **Limits.** A crash can be made on purpose: a state changed after a kill, before the next
  start, reads as a run cut off, unchecked, not as a break; the event says when and which state.
  The run cut off isn't named, nor its writes checked: nothing of it reached the log. A state
  moved after a deploy, a restore or a stop and run again is left to their events, as before.
- **Tests.** `a_run_a_crash_cut_off_before_its_line_is_said_and_a_state_changed_while_stopped_breaks`
  (a crash with and without a run cut off, a clean stop with one, a state changed once loaded, a
  second crash before any run), `a_clean_stops_mark_is_taken_by_the_next_start_alone`.

### G12. Reading a text doesn't double with how deep it nests

What reads a flow (`holon check`, `POST /deploy` and the preview, each run that reads a secret,
fmt, the editor and the language server) does work in proportion to the text, however its parts
are reused or nested (audit R17):

- **Where a secret goes** (`holon_core::taint`). The walk follows every path, since the host asks
  of a node by its path, so a function using one that uses one… twice each was 2^k paths: 18
  levels took 2.8 s to check, and each two more four times as long. The walks one question about a flow takes (each secret's,
  and what is answered to it) visit at most `MAX_VISITS`, 65,536 nodes, between them (`Visits`);
  past it a function not yet followed is taken as not followed, as a `map`'s function is: what it
  makes derived, every effect it could make carrying the secret, every host it could call sent it.
  That says more than the flow does, never less: the approval asks for more, and the log withholds
  more. The samples' most is 2,065 a question.
- **A sum known to be a number** (`numeric`, a collection's key): asked each sum once, without
  recursing; `a1 = a0 + a0`, `a2 = a1 + a1`… was asked again at each use.
- **fmt's layout** (Oxfmt's) lays a call's arguments out again for each way it tries to break
  them: `f(f(…))` 22 deep took a second, `xs.map((x) => String(…))` six deep 2.5 s and seven past
  half a minute. fmt refuses a text whose calls, templates and functions written in place nest
  more than 20 in each other (a function twice; a function's block starts again), saying where
  to name a part (`format::MAX_LAYOUT_NESTING`); at the bound the slowest form takes a few
  tenths of a second. The samples' most is 7. The editor's Format, the language server's and a
  canvas printed as text each ask fmt.

- **Limits.** A flow past the walks' budget is asked for more than it does (every secret it reads
  may answer its callers, here); a run of it walks about 0.1 s more before it starts, two
  questions' worth. A canvas whose nodes print as calls nested past fmt's bound can't be shown as
  text until a part is named. Looking a node up while lowering a text (`builder::data`, `source`)
  is a scan of the nodes and wires so far: quadratic in a very long handler, not doubling.
- **Tests.** `no_entry_doubles_its_work_with_each_level_a_text_nests` (every entry that reads a
  text, over each form that doubled, 20 to 60 levels deep, one with 100 secrets, in 20 s together;
  each fix taken out fails it), `the_samples_walk_far_below_what_a_question_may_visit`,
  `the_samples_lay_out_far_below_fmts_bound`.

### G13. A text is counted as the parser reads it

The count that bounds how deep a text nests (`lang::nesting`, 128 levels) is the only thing
between a text and the parser's recursion, so it reads a text's words, comments and line ends as
oxc's lexer does (audit R36). Where it read one otherwise it skipped what the parser read, or
began a statement where the parser went on one, and a text got past it as deep as it liked:
`x as⏎any as⏎any…` and long `else if` chains crashed `holon check` at 400,000 links, and an
agent's proposal 3,000 deep could trap a reviewer's tab, whose wasm has a 1 MiB stack.

- **Read alike.** A line ends at `\n`, `\r`, U+2028 and U+2029, ending a `//` comment, a string
  (at `\n` and `\r`) and a regular expression; a `\` before it goes on with a string. oxc's spaces
  (U+00A0, U+FEFF, U+3000, U+200B and the rest) are spaces, never words. `#!` at a text's start is
  a comment to its line's end. A word that wants what follows it (`as`, `keyof`, `in`) keeps its
  statement going over a line's end; each `else` is a level of its `if`, kept over the `;`s of
  the chain. TypeScript's `a!` is a value, and a `++` begun on a line is the next statement's.
- **Refused where it can't tell.** Whether a `/` divides or starts a regular expression hangs, after
  a `}`, an `of` or a `>` that may close a type's arguments, on what the parser made of what came
  before; read the wrong way, each skips what the other counts. The count refuses that `/`,
  saying to put one side in parentheses (`Refused::TwoWays`).
- **Read to its end.** `#` begins a word, as a private name's does (`#x`), and the word has it,
  so every step of the count reads something: at a `#` in code it read an empty word and the same
  place again, forever, and `a #b` hung `holon check`, the language server as `#` was typed and
  the editor's tab (`the_count_reads_every_text_to_its_end`, each ASCII character and a few past
  it in each place a token stands). The count's loop has a floor too: a turn that leaves the
  place as it was is refused there (`Refused::Stuck`, a fault of Holon's to report), so no kind
  of token can bring the hang back (audit R58).
- **Limits.** An `else if` chain counts three levels a link (its condition, its statement or
  block, its `else`): 41 links fit in a route's handler, where the parser's tree is one a link.
  `graph::graph` lowers on the deep stack, as `lang::compile` does.
- **Tests.** `no_way_of_writing_a_text_gets_it_past_the_count` (each way that was read otherwise,
  four times the bound deep by oxc's own tree, refused by every entry on 1 MiB of stack; each fix
  taken out fails its row), `no_text_nested_past_the_bound_crashes_any_compile_entry`.

## 3. Policy, not accident

Some choices weaken a record on purpose, and are listed so they are seen as choices (RA §2.5): a
`lazy` handler's record is on disk within a second; requests no route took are recorded lazily;
`allow_unrecorded` is on by default (each unrecorded handler is still approved one by one, and a
host can refuse them all).

## 4. How it was checked

1. **The audit** (RA §1): each change of `aa2704a..9969f8b` read for what it could cost when
   something goes wrong, sorted as a resilience cost, a drift risk (knowledge kept in two places)
   or sound; each fix one of three kinds: one source of truth, fail safe ("do the slower thing
   that was always right"), or a tripwire that fails when the fast path is left.
2. **Every fix measured** for its throughput cost, both ways (RA §2.6, §2.7, §2.9).
3. **The end-to-end suite** through the real editor and server: 72 passed, 1 skipped (MongoDB).
4. **Soaks**: a mixed load on one server for 22–25 minutes (unrecorded, lazy and strict reads,
   strict writes, compute, a route that always fails), sampled every 10 seconds, with a latency
   probe every minute. The first served 23.8 million requests, none failing that shouldn't;
   threads, open files, logs and latency stayed flat; memory rose from 311 to 660 MB, mostly the
   allocator's (flat with its adaptive thresholds pinned), and two steps that were R5. It found
   R5 and R6. See [Measuring](measuring.md#soaks).
5. **The pass over the JavaScript library's leaves** (`c865f3a..` on `library-leaves`): two readers
   given the code, a sweep of every pure leaf over hostile values on a host's 2 MiB thread
   (`audit_leaves`), the differential harness against `5f29d1d` (300,000 cases, none diverging),
   the workspace, wasm, editor and end-to-end suites, and a release server sent the requests that
   found each fix. What a few bytes of a request could make much more of is bounded where it was
   found: a replacer that wraps without end, a pad, repeat or replace past 2^24 units, a laid-out
   text, and a state write too deep to load at the next start, which would have kept the server
   from starting.

## 5. Open

- Fixed: **a value nested deeper than a run thread's stack can walk aborted the server**, every
  deployment with it, where a panic is answered 500 (G3). A flow nesting a list a level per step of
  a range its request sized did, at 10,000 deep, on a release server: `Timer::produced` turns a
  node's outputs into the log's form, recursively, to see if they fit (the same route unrecorded,
  keeping no outputs, answered), and core's own walkers recurse the same way (`add` of such a list
  overflows by itself). Found by the leaves' pass (§4), on main as well. A value now nests at most
  256 deep, however a run builds it ([Security §2.7](security.md#27-what-a-flow-can-reach)): the
  fold gives nothing past 256 steps, and the server answers on. Measured on 2 MiB, a route walks a
  list about 6,000 levels deep end to end in release and about 470 in a debug build, so 1,024
  would not have fit a debug build, nor a log line of records read back in release.
- Fixed: **a log line holding a value past 41 records deep didn't read back** (serde_json stops at
  127 levels; a record takes three in the log's form). A run's entry holds the state it began
  with, so one deep value stored made every later line unreadable, and a reopened log skipped them
  as cut short: their runs gone from reads, the line after them a break, and a rewrite after an
  interrupted cut-back dropped them. Nor did the deployments file holding a waiting run's event
  (on a server without a secrets key: it kept the server from starting), nor a waiting run's sealed
  part (the run wasn't opened). On main as well.
  They are read past serde_json's stop, bounded by a count of the brackets
  (`wire::read_json`).
- R8, fixed (RA §2.12): a log kept every run it keeps in memory, whole (1.1 to 3.6 times its
  lines), bounded by no cache's size; now a log on file keeps its newest 32 there and reads the
  rest from its file with its lock released: a server on the soaks' data folder starts at 39 MB
  instead of 320.

- R6's second half, fewer and larger files, done by packing: the values a sweep keeps past an
  hour are moved into packs ([storage L4 step 10](../../L4s/storage-L4.md)); the logs' older runs
  are sealed into gzipped segments (step 9). What a full disk does, the backup and the warnings
  are in [Storage](../capabilities/storage.md).
- The review of `aa2704a..09f0bdc` by a reader given only the code, done: ten defects, each fixed
  with a test that fails without it; what they teach is in
  [Lessons](lessons.md#what-the-review-found).
- Object storage for the backup (storage step 7).
- Gaps in the tests ([Testing](testing.md#4-gaps)): nothing in CI checks the caches against real
  memory, nor that an unrecorded route reading state goes to the pool, nor the soak's flat lines.

## 6. History

| Commit | Change |
|---|---|
| `c4ab818` | The audit |
| `dc21f76`, `ca5f91c` | R1–R4 (panic guard; only a run that can't wait on the worker; the evaluator fails closed; cut-backs outside the lock) and their costs |
| `cf1f961` | D1–D3: one list of primitives, one counting rule, the redaction shortcut checked |
| `4bb40a8` | An unrecorded run is never logged, failed or not |
| `9969f8b` | D4, D5, S1, S4, S5: tripwires; the caches count memory; the value cache's size a setting |
| `655e35b`, `58b87e6` | The end-to-end suite; the first soak, finding R5 and R6 |
| `42a75f9`, `6d764f5` | R5 (the folders' sweep holds up no run), R6 (the server sweeps its store) |
| `de42b69` | The server's own sweep paces its removals |
| `824db82` | R8: a log keeps only its newest runs in memory |
| `5b921ed`…`09f0bdc` | Storage steps 1–6, 8–10: the sweep a setting, the host's view and each flow's, warnings, `when_full`, the backup, packs, sealed segments ([Storage](../capabilities/storage.md)) |
| `7c0b545`…`9b4bd37` | The review's ten fixes, and the backup's files named `holon-archive-` |
| `1c1d2ee`…`aca0f08` | The pass over the library leaves (§4.5): what a few bytes make bounded (a text, a replacer, a laid-out text, a state write), `unique`'s hashing, a reader panic, the leaves' sweep; a value nested past a thread's stack found, and open (§5) |
| `bd0fa4c`, `eeaab65` | A value nests at most 256 deep, and the JSON Holon writes holding one reads back: the abort and the unreadable log lines fixed (§5) |
