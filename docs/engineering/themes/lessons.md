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
- **Background work is measured under load too.** A sweep is free when nothing is there; on a used
  store it took 50 seconds and its memory stayed with the process.

## How the work was run

- **One step at a time, each measured and committed on its own**, so a step that costs more than
  it gives can be taken back alone, and a reader can see what each one bought.
- **Read the work back with fresh eyes.** The audit went over every change for what it could cost
  when something goes wrong, rather than what it gained; it found four costs and five places
  where knowledge had been put in two. A review by a reader given only the code is next.
