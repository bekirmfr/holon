# Lessons

What the performance and resilience work taught, written down so it isn't learned again. Each
lesson names the case it came from; [Performance](performance.md), [Resilience](resilience.md)
and [Measuring](measuring.md) hold the details.

## Measure, then choose

- **The cost is rarely where it is guessed.** A request read its 19 unit files under the store's
  lock (C1: 199 µs at one connection, 34 ms at 64); formatting hex a byte at a time was a quarter
  of a short run (12.4a); a tenth of the time under the log's lock was freeing the entry let go of
  (12.4c); strict's fsync was four times the 150 µs first guessed (Q6). None of these was the
  first suspect. Profile, read where the instructions go, then change that.
- **A measurement decides what not to do too.** Shared values in the evaluator (step 4) were
  planned; measured after step 3, copying was 5–10% of a run, for a change touching about 850
  places. Deferred. Reserving capacity ahead in small vectors, tried during step 6, made runs
  slower and was taken out. A gain that can't be measured is not taken.
- **Count in the unit that matters.** The caches counted the bytes of an entry's encoding, which
  is not what it takes in memory: they held 2.8 and 7.8 times their sizes (S1). Found only by
  measuring memory itself.

## Speed has a reach beyond its route

- **A fast path's cost lands on someone else.** Running short routes on the data plane's worker
  (12.1) made `hello` faster; it also let one deployment busy reading its notes take an unrelated
  route from 25,000 requests a second to 30 (R2). Its rule was "short and read-only"; the right
  rule is "can never wait". When a change makes something cheaper by sharing, ask what the
  sharing does when one side is slow.
- **A speed-up can carry an old cost further.** The log's cut-back under its lock predated the
  throughput work; the inline path made its 100–150 ms stalls reach other deployments (R4).
- **Safety bought speed back.** Taking waiting off the shared workers and cut-backs off the
  runs (R1–R4) cost the pool's hop on routes that may wait, and made the `notes` listing 39–46%
  faster at 64 connections and strict `hello` 10–18% faster. A queue of runs behind a stall is
  slow as well as unfair.

## The record is the product

- **Most of a request's cost is its record, and that is the point.** Holon strict keeps up with a
  C# service keeping the same record (77–96% of it); the gap to plain .NET is the record, not the
  language. A faster path that keeps less (dropping each step's output, Q16, 40% faster) is a
  change to what Holon promises, not an optimisation, and was not taken.
- **Weakening a record is a person's choice, written where they see it.** `lazy` and
  `unrecorded` are words on a handler, approved at deploy; the defaults that weaken records are
  listed as policy (RA §2.5).
- **"Unrecorded" means unrecorded.** Logging a failed unrecorded run so it could be debugged
  (Q10) undid the reason it was declared (keeping requests out of the log), turned failures anyone
  can force into extra runs and lines, and answered from a second run. A declaration is a reason;
  count instead, and let the author make the handler `lazy` to look.

## Correctness first, then speed

- **Golden records before touching the evaluator.** `7eb8e22` recorded every answer, record and
  hash before step 1; every step since changed none of them, and the one planted change was
  caught.
- **A fast path is tested against the plain one.** The compiled plan against the walk on 3,000
  random graphs; the indexed cut-back against the one that parsed; the redaction shortcut against
  redacting, on every logged run in a debug build. Writing the plan's test found a bug the
  samples never reached (R3).
- **A silent fallback hides a regression.** A plan that hands over to the walk is still right, so
  every test passes when the plan stops being used. A tripwire fails instead (D5).
- **Knowledge in two places drifts.** The primitives' names in two lists meant a name in one only
  was a node that computes nothing (D1); the counting rule written twice (D2). One list, or a
  test that checks one against the other.
- **Fail closed.** A case the code doesn't expect answers absent or 500, never a guess and never a
  panic that drops the connection (R1, R3).

## Simpler isn't always cheaper

- **The obvious design was measured and was worse, twice.** For R4, letting the run that found
  the log full cut it held that run 151–163 ms at p99; planning the cut and making it up to a
  second later copied everything written meanwhile under the lock, 371–408 ms. Making it on the
  sync thread, catching up with the lock released until little was left, took the stall away.
  Measure each design, not just the first.

## What only time shows

- **A six-second benchmark can't see what happens every ten minutes.** The soak found the folders'
  sweep reading every whole log under its lock (R5) and the store growing 5.4 GB an hour with
  nothing to sweep it on its own (R6); neither shows in a short run, and R6 predates all of this
  work. Run long, on a used data folder, and watch what grows.
- **Memory that rises isn't a leak until it rises with the allocator pinned.** glibc keeps what it
  is given back; pinned, the soak was flat.
- **Background work is measured under load too, against a control.** The second soak showed the
  `notes` routes 17% slower. The first guess (the sweep pushing hot values out of the cache) was
  fixed and measured: no change. Measured instead with a control (no sweep) and a dry run (all the
  reading, no removing): the reading cost nothing, the removing took the listing from about 380
  requests a second to 123. Pacing the removals brought it to a 7–12% dip. A fix for a guessed
  cause, measured and found to change nothing, is taken out.

## How the work was run

- **One step at a time, each measured and committed on its own**, so a step that costs more than
  it gives can be taken back alone, and a reader can see what each one bought.
- **Read the work back with fresh eyes.** The audit went over every change for what it could cost
  when something goes wrong, rather than what it gained; it found four costs and five places
  where knowledge had been put in two. The review after the storage work, given only the code
  (`aa2704a..09f0bdc`, 67 commits), found ten more, below: each in code every test and live run
  had passed.

## What the review found

Ten defects in the storage and log work, each fixed in a commit of its own with a test that
fails without it (`7c0b545` to `a6babdf`, then `9b4bd37`). What they have in common is worth more
than any one of them.

- **A folder the server shares with people is never its own.** The backup's housekeeping took
  every `*.jsonl` under the backup folder for a batch of runs, "exported" it and removed it;
  `export_of` leaves out lines that aren't log entries, so a file of the admin's own on the drive
  chosen as the backup was exported empty and deleted (`7c0b545`). A server touches only what it
  can tell is its own by name: every file it writes there now starts `holon-archive-` and has a
  batch's exact shape, and nothing else is exported, removed or counted (`9b4bd37`). Ask of every
  delete: could this path be someone else's?
- **The undo path needs the tests the do path got.** `when_full` paused at `full_at` and resumed
  under `warn_at`; written in different units (85%, and 500G free), `full_at` can be reached
  first, and every flow was paused and resumed in the same look, every ten seconds (`ecc92ce`).
  Undoing a roll raised the log's keep, which opened the log again from its file and forgot that
  its newest lines weren't synced: a strict run was told its record was on disk when it wasn't
  (`ca69399`). Both were in the half of the feature that runs when things get better, which the
  tests had looked at least.
- **Two paths to one effect drift apart.** A log cuts back holding its lock or with it released
  (R4); only the first cleared the backup's error, so once a backup failed the host warned of it
  for good (`f288afe`); and only the first had been thought through for a failure after its
  segment was renamed in, so the second left a segment nothing knew of (`d896a4c`). The same
  lesson as one source of truth (D1, D2), for behaviour: where two paths must end alike, make
  them share the ending, or test each.
- **A later step moves what an earlier one measured.** Sealing (step 9) left a log's file its
  newest 128 runs and put the rest beside it, but `roll` (step 5) still picked the biggest log by
  its file, and the host's view still counted logs by their files (`7a0e26a`). When a change
  moves where data lives, find every reader of the old place.
- **Handed to another thread is not done.** `roll` asked for a cut and swept at once; the cut was
  made by the periodic sync up to a second later, so the sweep a full disk waited for still saw
  the runs it was to free (`159a6db`). An action queued for later has not happened; what depends
  on it waits for it, or does it.
- **One bad file mustn't stop the server.** A pack that couldn't be read failed the whole store's
  opening, every other value intact, and a test asserted that it should (`91cc5e2`). A sealed
  segment that can't be read was already skipped and said. A test can hold a wrong decision as
  firmly as a right one; on start, say what is broken, leave it to be restored, and run.
- **One failure mustn't stop a loop.** The backup's housekeeping returned at the first batch it
  couldn't export, so one bad batch held up every batch after it, for good (`7eebc94`). Note the
  failure, go on with the rest, and keep warning.
- **Write aside, then swap under the lock: everywhere.** R4 took a log's cut-back out from under
  its lock; packs still wrote and synced a whole pack holding the index's lock, and the sweep held
  the store's lock across a pack's rewrite, stalling every read and put meanwhile (`a6babdf`). The
  pattern found once is looked for in the code written after it.
- **A fix's test is run against the code before the fix.** Two of the ten tests passed without
  their fix as first written: one because a later step of the test made up for the bug, one
  because the failure it forced came a step before the one it was meant to test. Each was
  changed until it failed without the fix and passed with it.

