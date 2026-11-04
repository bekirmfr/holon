# Request path: a run pays for its own flow, not for the server (L4)

**Status:** proposed (2026-11-02). Nothing here is built yet. Every claim about the code is cited
by file and line at `aa2704a` (`main`), and every number was measured on a release build of that
commit (Appendix A). §3 is measured; §4's "Results" say for each figure whether it is measured or
estimated. Appendix B lists problems found along the way that this note doesn't set out to fix.

A deployed flow answers one request in **0.5–0.8 ms**. Under load, its throughput **stops at about
1,000–1,700 requests a second however many connections there are**. A stateful flow, on a server
with 33 deployments, manages **53–65 a second**, with a median answer near **one second**. The
flow's own work is not the reason: a run computes in 0.1–0.4 ms. The rest is work the server does
around each run:

1. It **reads the whole flow from disk again** on every request, one file per unit, through a
   lock the whole store shares (§2.1).
2. It **writes the run record while holding the lock on the table of all deployments**. Every 500
   runs it also re-reads, re-parses and rewrites the whole log file there (§2.2).
3. For a stateful flow, it **rewrites and fsyncs the whole deployments file** (every deployment,
   with its document and kept versions) on every run, even one that only read its state (§2.3).

This note makes a request's cost depend only on its own flow and its own deployment:

> **Requests to different deployments never wait for each other, and no request does work that
> grows with the number of deployments or with the runs before it.** A run costs its evaluation,
> one appended log line, and the writes that make its record (and any state it changed) durable.

It doesn't change what a run records, the file formats a person reads, or the evaluator. It makes
the record stronger in two ways:

- **A run's record is on disk before its answer, by default (C5).** Today it never reaches the disk
  on purpose: the log is never fsynced, so a power loss can lose run records whose state changes
  survived, and can leave a witness holding heads the disk lost (Appendix B.5).
- **Each handler (route, clock, file handler, action) gets a log level.** The default is `strict`.
  A handler that only reads may be declared `lazy` (its record reaches the disk within about a
  second, C2 d), for speed. With a person's approval, it may be declared `unrecorded`, to keep
  noise out of the log: one summary row instead of a run per request, with failures still
  recorded.

The level is part of the flow's text and hash, it's shown at approval, and the host's policy caps
it (C5).

## Contents

1. Where Holon stands
2. What a request does today
3. Measured: what each cost is worth
4. The changes (C1–C5), each with what it touches, results and side effects; §4.5, the per-run constant that remains
5. Lock order, before and after
6. Build order
7. Samples
8. Open questions
9. Invariant check
- Appendix A. How it was measured
- Appendix B. Found along the way

## 1. Where Holon stands

This is a reference point, not a target. The same endpoints run as Holon samples and as a .NET 8
minimal API (Kestrel, `CreateSlimBuilder`, logging off), on the same 4-vCPU machine, one server at
a time, loaded with `wrk` over loopback (Appendix A). It was measured twice:
- **before** this note's changes, at `aa2704a`, when every Holon run was recorded and synced
  lazily, and the strict and unrecorded columns were throwaway builds standing in for the design;
- **after** them, at `75a59c7` (build step 12 done), with the levels built (C5): **strict**, the
  default, on disk before the answer; **lazy**, synced within a second; **unrecorded**, counted and
  not logged. Two rounds of everything; ranges are the two rounds.

Ratios are taken within one run. Plain .NET itself measured 96,980–116,612 req/s on `hello` before
and 132,797–133,151 after, on the same machine: the machine's own noise.

### 1.1 Plain .NET against Holon

| | .NET 8 | Holon before (`aa2704a`) | Holon now, strict (default) | Holon now, lazy |
|---|---|---|---|---|
| `GET /hello/:name`, 64 connections | 132,797–133,151 req/s, p50 0.35 ms | 1,670 (58× slower) | 8,787–9,470 (14–15×) | 14,912–15,717 (8.5–8.9×) |
| `POST /calc`, 64 connections | 119,700–125,496, p50 0.40 ms | 1,020 (79×) | 7,414–8,098 (15–17×) | 11,312–12,486 (10–11×) |
| `GET /hello/:name`, p50 at 1 connection | 49–66 µs | 556 µs (5×) | 419–427 µs (6.4–8.7×) | 111–112 µs (1.7–2.3×) |

Still the least fair pairing: the C# service keeps no record. Before, Holon ran **1,480 req/s at
1 connection and 1,670 at 64**: more concurrency bought nothing, because something on the request
path ran one request at a time (§2). Now strict `hello` goes from about 1,700 at one connection to
9,000 at 64, and lazy to 15,000. The one-connection latency of strict is the disk: it waits for its
record's sync, which plain .NET never does (§1.3 pays the same wait in C#).

### 1.2 Unrecorded Holon against plain .NET

An `unrecorded` run (C5) keeps no record, like the plain C# service, so this is the bare cost of
answering. Before, Holon stood in for it with a throwaway build (`cache,nolog,nopersist`, §3); now
it is the built level, approved per handler:

| 64 connections | .NET 8 | Holon unrecorded now | Gap now | Gap before (stand-in) |
|---|---|---|---|---|
| `GET /hello/:name` | 132,797–133,151 req/s, 16–17 µs CPU/req | 48,355–49,020, 63–65 µs | 2.7× throughput, 3.9× CPU | 4.2×, 6.0× |
| `POST /calc` | 119,700–125,496, 19–20 µs | 47,744–50,153, 63–67 µs | 2.4–2.6×, 3.3× | 4.6×, 6.3× |
| `GET /last` (`counter`; reads state) | 133,082–144,807, 15–17 µs | 41,361–41,791, 70 µs | 3.2–3.5×, 4.1–4.7× | 6.1×, 8.5× |
| p50 at 1 connection: hello, calc, last | 49–66, 70–72, 40–68 µs | 76–79, 95–98, 79 µs | 1.2–2.0× | 3.3–7.8× |

For bare request handling .NET is now 2.4–3.5× more efficient, from 4–6×: compiled code against a
graph evaluated node by node. Holon's extra 45–55 µs of CPU per request, from 100–140, is the
graph's evaluation, the permit checks and the run's setup. At one connection an unrecorded `hello`
spends 48 µs in Holon's handler (a throwaway probe), and its graph's evaluation is about half of
that (`eval_observed`, 51% of the handler's instructions under callgrind, much of it allocating and
copying values and ordering the graph on every run); the run id's hash is a tenth, and the rest is
the run's bookkeeping: the permit, the sourced values, the request record. The interpreter is the
largest single part of this gap, and what a compiled plan (§4.5) would take off. What came off it (§4.5, build step 12):
the flow cache (C1), the state no longer copied to be read (C3), a short route run on the worker
that read its request instead of a blocking thread (12.1), no timeline for an unrecorded run (12.2),
hashes written without a `format!` a byte, and a permit's names worked out once (12.4).

**Since then**, after the interpreter's steps 1–3 (interpreter-path-L4.md §4): unrecorded `hello`
answers 65,536–68,300 req/s at 43–46 µs of CPU a request, against .NET's 132,607–134,479 at 17 µs
in the same rounds, **2.0×** its throughput (from 2.7×), and `calc` 62,595–62,872 at 48–49 µs.
After the per-request bookkeeping (interpreter-path-L4.md §4, item 6): 73,041–77,022 req/s at
37–39 µs against .NET's 127,458–134,524 at 16–18 µs, **1.75×**; strict `hello` 10,513–10,727, above
the C# service keeping the same record in §1.3 (9,843–10,268, not measured again then).

### 1.3 Strict Holon against a C# service with the same record

What Holon does on every recorded run is give the run a replayable, hash-chained record that is on
disk before its answer (`strict`, the default, C5). To compare like with like, the C# counterpart
gets a middleware that does the same on every request:
- it buffers the request and the answer, and checks a grant;
- it builds a 4.6–4.8 KB record of the shape Holon writes: the request with its headers and body,
  the sourced values, the outputs, the answer, a 19-node timeline with each node's outputs, the run
  id, unit, nonce and grant. Holon's own records are 4.4 KB (`hello-api`) and 5.9 KB (`calc`);
- it scans the record's bytes for a secret's value, standing in for redaction;
- it chains the record to the one before with SHA-256 (Holon uses BLAKE3, `recorder.rs:50`) and
  appends it with one positional write;
- a strict run then waits for an `fdatasync` covering its line, with the same group commit as C2 (e).

A check confirmed that the C# records are valid JSON and correctly chained.

| 64 connections | .NET strict | Holon strict now | Before: design proxy | Before: the log lock |
|---|---|---|---|---|
| `GET /hello/:name` | 9,843–10,268 req/s | 8,787–9,470 (86–96%) | 97% | 50% |
| `POST /calc` | 9,504–9,655 | 7,414–8,098 (77–85%) | 95% | 38% |
| `GET /last` | 10,719–10,886 | 8,662–9,095 (80–85%) | 98% | 60% |
| CPU per request: hello, calc, last | 192–203, 213–217, 176 µs | 229–247, 274–298, 249–261 µs | | |
| p50 at 1 connection: hello, calc, last | 427–437, 443–454, 428–436 µs | 419–427, 457–467, 430–436 µs | | |

Before, the two Holon columns bracketed the design: the proxy appended a 5 KB line and synced it
without building the real record, and this note expected the design to land 10–25% behind .NET
rather than 2–5%. **Built, it lands 4–23% behind**, `hello` closest: the record's own work (the
entry, its timeline of each node's outputs, redaction, the chain) is the difference, 35–85 µs of
CPU per request more than C#'s, and the disk sets the pace for both. Holon gives that record with
no code; the C# middleware is about 130 lines. **At one connection the two are alike** (the table),
the sync about 60% of the request in both: in a strict `hello`, 259 µs of 436 for Holon's
`fdatasync`, by a throwaway probe, beside 50 for the run, 48 for its line and 38 for the hop to the
blocking pool and back. A first measurement had Holon 150–260 µs slower there; it was taken right
after a 64-connection run, while the disk was still busy with what that run wrote. Measured on its
own the gap is gone. (Syncing through a handle kept open, instead of opening the log's file for each
sync, was tried on the way and changed nothing measurable: not kept.)

### 1.4 What a C# record costs, step by step

`hello`, 64 connections, with the C# middleware stopped after each step (`RECORD=plumb`, `build`,
`lazy`, `strict`), measured before:

| Step | Throughput | CPU per request |
|---|---|---|
| plain | 119,912 req/s | 18 µs |
| + buffer the request and answer, and check the grant | 103,125 | 22 µs |
| + build the record (`Utf8JsonWriter` into a reused buffer) and scan it | 62,915 | 45 µs |
| + chain it and append it (one lock, SHA-256, one `pwrite`) | 22,501 | 125 µs |
| + wait for `fdatasync` (strict) | 9,334 | 211 µs |

A first, quick C# recorder (reflection-based serialization of anonymous objects, LINQ, string
concatenation) cost nearly twice as much (12,130 req/s with no fsync, 5,938 strict). The tables use
the rewrite, so the comparison isn't against a strawman.

The C# record appended without fsync (`lazy`) is Holon `lazy`'s counterpart. Now, at 64 connections:

| | C# lazy | Holon lazy |
|---|---|---|
| `GET /hello/:name` | 28,190–41,949 req/s, 77–108 µs CPU/req | 14,912–15,717, 172–174 µs |
| `POST /calc` | 16,779–33,740, 82–157 µs | 11,312–12,486, 210–237 µs |
| `GET /last` | 16,355–17,052, 147–151 µs | 16,774–16,965, 161 µs |

The C# figures vary most between rounds. Holon lazy, estimated at 15,000–16,000 before it was
built, measures 14,900–15,700 on `hello`, and matches C# on `/last`. What it spends beyond C# is
the record's own work again: the timeline of every node's outputs is most of the line (§8 Q16 keeps
it), and the line is hashed and appended under the deployment's log lock, one run at a time (C2;
build step 12.4 c took the rest out of it).

### 1.5 Holon against workflow platforms: n8n and Node-RED

The same two routes built in the two best-known flow platforms: n8n 2.46.4, a workflow automation
tool, and Node-RED 5.0.9, a flow-based programming tool. Both ran on Node.js 24 on the same machine,
one server at a time (Appendix A). Holon is the build after interpreter step 1 (`440eca0`).

- **n8n:** a Webhook node answered by a Respond to Webhook node. Its text is an expression: the
  greeting, or calc's arithmetic as a chain of ternaries, the nearest n8n has to a `match`. A
  third route does calc in a Code node instead, the way n8n users write logic.
- **n8n's settings that matter here:** run saving on (`EXECUTIONS_DATA_SAVE_ON_SUCCESS=all`, the
  default, into its SQLite database) and off (`none`). The expression engine is `vm`, the 2.x
  default (each execution borrows a V8 isolate from a pool of one), and was also run as `legacy`,
  1.x's in-process evaluation and n8n's fastest.
- **Node-RED:** `http in`, a function node, `http response`. It keeps no record of runs.

| 64 connections, two rounds | `hello` req/s | `calc` req/s | CPU a request, `hello` | p50 at 1 connection, `hello` |
|---|---|---|---|---|
| Holon strict | 8,786–9,515 | 7,827–8,567 | 225–246 µs | 588–606 µs |
| Holon lazy | 14,694–16,192 | 11,378–12,453 | 165–177 µs | 107–115 µs |
| Holon unrecorded | 55,808–58,965 | 54,794–55,107 | 52–54 µs | 71 µs |
| Node-RED (no record) | 7,052–7,239 | 8,353–9,137 | 144–150 µs | 251–264 µs |
| n8n, saving on, `vm` (as shipped) | 32 | 34 | 67–69 ms | 52.5–54.3 ms |
| n8n, saving off, `vm` | 35–38 | 36–40 | 61–65 ms | 49.0–52.8 ms |
| n8n, saving on, `legacy` | 56–59 | 69–71 | 16.9–17.4 ms | 13.4–13.7 ms |
| n8n, saving off, `legacy` | 60–66 | 72–80 | 15.1–16.0 ms | 11.9–13.3 ms |
| n8n, calc in a Code node | | 32–35 (`vm`), 62–72 (`legacy`) | 15.6–18.0 ms (`legacy`) | |

- **Against n8n, Holon is two to three orders of magnitude ahead.**
  - The closest pair is n8n saving its runs against Holon lazy, each keeping a record without
    waiting for the disk. n8n's fastest setting of that pair (`legacy`) answers `hello` at 56–59
    req/s; Holon lazy is 260–275× its throughput, at a hundredth of its CPU a request.
  - Holon strict, which waits for the disk before every answer, is still 150–170× that, and
    275–300× n8n as shipped.
  - At 64 connections n8n's p50 is about a second: it is one thread at about 16–70 ms of CPU a
    request, so requests queue.
- **Where n8n's time goes.** A CPU profile under load (`node --cpu-prof`, `legacy`, saving off)
  puts 43% of its busy time in the database layer, typeorm and SQLite, even with saving off: each
  webhook execution reads its workflow and writes and removes an execution row. The isolate
  engine (`vm`) adds 45–50 ms a request on top. Building the workflow object and its
  parameters is another 6–10%.
- **n8n's own figure.** Its documentation cites up to about 220 executions a second for one
  instance, on its own hardware, and scales out in queue mode: Redis and worker processes, each a
  process like this one. That is 3–4× what this machine gave n8n's fastest setting, and Holon lazy
  here is still 65–75× that.
- **Against Node-RED,** the fair pair is Holon unrecorded, since neither keeps a record. Holon
  answers 6–8× the requests at a third to a half of the CPU a request (52–57 against 111–150 µs),
  and Node-RED's single thread caps it near one core. Node-RED's latency at one connection is
  3.5× Holon's.
- **What this doesn't say.**
  - n8n and Node-RED are integration tools with hundreds of service nodes, an editor of their own
    and, in n8n, retries, credentials and a run history to browse. Neither aims at the request
    path, so this measures them where Holon is strongest.
  - n8n's saved run is a row of the execution's data, not a hash-chained record that replays the
    run. Whether it is on disk before the answer is SQLite's setting, not a level n8n offers.
  - The machine is the 4-vCPU VM of §1, shared with `wrk`.

### 1.6 What the comparisons don't cover

- The machine is a 4-core VM shared with `wrk`, so absolute numbers are low, and the same build
  varies between runs (plain .NET 97–133k on `hello`). CPU per request at 1 connection is inflated
  by .NET's thread-pool spinning, so the tables compare CPU at 64.
- The before columns of §1.2 and §1.3 are throwaway builds standing in for C1–C3 and C5; the now
  columns are the design built, at `75a59c7`.
- The C# timeline's values are filled in to match the size and shape. C# runs no graph, so it has
  no nodes to time, and its record can't replay a run.
- State writes (`counter /hit`) aren't compared. A C# counterpart would need durable state too;
  Holon's `/hit` is about 700–1,400 req/s, bound by each write's own sync of its state head (C3).

## 2. What a request does today

The data plane is `hyper` on one shared tokio runtime (`dataplane.rs:68`), with the default
blocking pool (up to 512 threads). Each request runs `serve_data_forwarding` on that pool
(`dataplane.rs:295`), which reaches `serve_flow` (`listeners.rs:76`). In order:

| # | Step | Code | Lock held | Measured cost |
|---|---|---|---|---|
| 1 | Check the caller's origin (clones the deployment's `browsers`) | `allowed_origin`, `listeners.rs:26`, from `dataplane.rs:291` | table, briefly | small |
| 2 | Look up the deployment, clone its handles | `listeners.rs:84–89` | table, briefly | small |
| 3 | **Load the flow's closure from the store** | `materialize_closure`, `units.rs:159`, from `listeners.rs:95` | **store mutex, once per unit** (`main.rs:300`) | 199 µs at 1 connection; **34 ms** at 64, mostly waiting for the lock |
| 4 | Evaluate the run: rounds, effects, and for state the optimistic commit | `data::handle` → `fire_with_state`, `firing.rs:120`, from `listeners.rs:148–153` | the state cell at commit (`firing.rs:203`), or for the whole of a last attempt (`firing.rs:270`) | 0.13 ms (`calc`) to 0.36 ms (`hello-api`) median, from the log's `duration_ms` |
| 5 | Count the trigger | `record_trigger`, `deploy.rs:867`, from `listeners.rs:207` | table, briefly | small |
| 6 | **Redact, number, chain and append the run record** | `log_run`, `firing.rs:786` → `Log::push`, `recorder.rs:497` | **table, for all of it** | about **150 µs** per run (§2.2) |
| 7 | **Persist the deployments table** (only if the deployment has state, or a run parked) | `persist_deployments`, `persist.rs:22`, from `listeners.rs:227–229` | `PERSIST` (global), then the table while it snapshots, then each cell, then the store | an fsync'd rewrite of the whole file: **80 KB with 3 deployments, 1.1 MB with 33** |
| 8 | Answer | back in `answer`, `dataplane.rs:263` | none | |

The other run kinds follow the same shape. Each loads its closure, runs, calls `log_run`, and (for
state or parked runs) calls `persist_deployments`:

| Run kind | Load closure | `log_run` | `persist_deployments` |
|---|---|---|---|
| Request | `listeners.rs:95` | `listeners.rs:226` | `listeners.rs:228` |
| Clock tick | `scheduler.rs:125` | `scheduler.rs:160` | `scheduler.rs:197` |
| File arrival | `arrivals.rs:373` | `arrivals.rs:407` | `arrivals.rs:412` |
| `/trigger` | `deploy.rs:924` | `deploy.rs:955` | `deploy.rs:958` |
| Action, and flow calls (`calls.rs:86` → `run_action`) | `deploy.rs:1079` | `deploy.rs:1119` | `deploy.rs:1121` |
| Resumed (durable) run | `durable.rs:637` | `durable.rs:651` | `durable.rs:684` (always) |

### 2.1 The closure is loaded on every request

`materialize_closure` (`units.rs:159–184`) walks the units from the root hash. It fetches each one
with `store.get` (`units.rs:168`), and `SharedStore::get` takes the single `Mutex` around the
whole store (`main.rs:300–302`). `FileStore::get` (`holon-host/src/lib.rs:665–672`) reads the
unit's file and checks it against its hash. `hello-api` has 19 units, and `strace` of one request
shows exactly those 19 opens.

The result can never change for a given hash. A unit's hash is its content, and **units are never
deleted**: the sweep removes values only ("Only values are swept, never the units (code) beside
them", `sweep.rs:21–22`). The work is repeated for nothing, and because it runs under the store's
global lock it also serializes requests across every deployment.

### 2.2 The run record is written under the table lock

`log_run` (`firing.rs:786–799`) takes the table lock (`firing.rs:787`) and, while holding it:

- builds a `Redactor` from the deployment's secrets (`Redactor::of`, `secrets.rs:852`, which
  locks the vault) and walks the whole entry with it, including every node's outputs in the
  timeline;
- computes channel packs (`Channels::packs_of`, `channels.rs:233`), using
  `dep.log.next_seq()`;
- calls `Log::push` (`recorder.rs:497–516`), which serializes the entry (4–6 KB here), hashes the
  line (`line_hash`), queues the head for the witness (`Log::export`, `recorder.rs:596` →
  `heads::note`, `heads.rs:76`), trims the in-memory entries (`prune`, `recorder.rs:470`) and
  writes the line (`keep_line`, `recorder.rs:521–549`).

`keep_line` calls `create_dir_all`, `path.exists()`, `OpenOptions::open` and `write_all` **on every
line**. Every `keep` lines (500 by default) it calls `cut_back` instead (`recorder.rs:554–593`).
That reads the whole file (`read_to_string`), **parses every line** into a `LogEntry` to get its
`at_ms`, joins the kept lines, and writes them with `write_private` (`sealing.rs:274–299`: temp
file, `sync_all`, rename).

Time inside the table lock per `hello-api` run, from timers in a throwaway build:

| Part | Average |
|---|---|
| Redaction and packs | 6–11 µs |
| Serialization | 13–23 µs |
| Append (`create_dir_all`, `exists`, `open`, `write`) | 20 µs |
| **Cut-back**: one every 500 runs, each taking **31–36 ms** (3.5 MB file); spread over the runs, about | 66 µs |
| Total held | **about 150 µs** (130–190 µs) |

So the table lock alone caps the whole server near 6,000 runs a second. Every cut-back also stops
**every deployment's** requests for about 33 ms.

This is a regression. `PLATFORM_LAYOUT.md` §7a recorded "disk I/O and per-deployment locks happen
under the global table lock, so one slow fire stalls every endpoint", and M0 marked it fixed ("no
disk I/O or per-deployment locks under the table lock ✅", `PLATFORM_LAYOUT.md:711`). The state
cell still follows that rule (`main.rs:420–432`; `JOURNAL.md:3951`, "Correction #2"). The run
log, added later, brought disk I/O back under the lock.

### 2.3 A stateful run rewrites every deployment

`persist_deployments` (`persist.rs:22–92`) runs after every run of a deployment with state, through
the per-run call sites in §2's second table, including runs that only read. It:

- takes a process-wide `PERSIST` mutex (`persist.rs:23,27`), so every stateful run of every
  deployment passes through it one at a time;
- under the table lock (`persist.rs:28–64`), clones **every** deployment's record: document,
  `versions_kept`, grant, sessions, and parked runs (sealed, which takes each deployment's secrets
  lock at `persist.rs:58`);
- for **every** deployment with state, calls `State::save` (`persist.rs:71`; `holon-host/src/state.rs:103`).
  That is a `put_value` per key, and each `put_value` at least stats the value's file under the
  store mutex (`holon-host/src/lib.rs:673–681`);
- serializes the table as pretty JSON and writes it with `write_private` (fsync, rename,
  `persist.rs:83–89`).

The cost grows with the server, not with the run:

| `counter` sample, 64 connections | 3 deployments (80 KB file) | 33 deployments (1.1 MB file) |
|---|---|---|
| `GET /last` (reads state) | 479 req/s, p50 127 ms | **66 req/s, p50 936 ms** |
| `GET /hit` (writes state) | 396 req/s, p50 153 ms | 65 req/s, p50 933 ms |

### 2.4 The state write path

A run that writes state commits under its cell's lock (`firing.rs:203–237`). It applies its writes
(`commit`, `firing.rs:468`), then calls `snapshot_after` (`firing.rs:230`; on a last attempt,
`firing.rs:298`) → `save_snapshot` (`restore.rs:23–32`). That puts each changed value and a
snapshot record into the store. `FileStore::put_value` writes a new file with `write_atomically`
(`holon-host/src/lib.rs:642–658`: temp, `sync_all`, rename) **while holding the store mutex**
(`main.rs:303–308`). So one deployment's state write stops every other deployment's store access
for the length of an fsync.

### 2.5 Two queues in a row

The table in §3 shows that removing either of §2.1 or §2.2 **alone** helps little. Requests pass
through the store lock and then the table lock, so throughput is set by whichever is slower, and
both have to go before the server scales with its cores.

## 3. Measured: what each cost is worth

The throwaway build gave `serve_flow` three switches, read from `HOLON_X`:

- `cache`: keep each closure in memory by root hash;
- `nolog`: skip `log_run`. This is an ablation to price the log, **not** a proposal; C2 keeps
  every record;
- `nopersist`: skip the per-run `persist_deployments`.

All runs used one server with 33 deployments, 64 connections, 6 s each, after a 3 s settle
(req/s · p50):

| Switches | `hello-api` | `calc` | `counter` `/hit` (writes) | `counter` `/last` (reads) |
|---|---|---|---|---|
| none (today) | 1,575 · 39 ms | 1,014 · 60 ms | 65 · 933 ms | 53 · 1.00 s |
| `cache` | 3,820 · 12.6 ms | 3,076 · 15.2 ms | 67 | 61 |
| `nolog` | 1,778 · 35 ms | 1,077 · 57 ms | 63 | 53 |
| `nopersist` | 1,418 · 43 ms | 1,009 · 61 ms | 863 · 69 ms | 879 · 72 ms |
| `cache` + `nolog` | **21,044 · 2.8 ms** | **16,285 · 3.6 ms** | 65 | 52 |
| all three | 19,279 · 3.0 ms | 16,314 · 3.6 ms | **1,453 · 40 ms** | **14,691 · 4.0 ms** |

At 1 connection, `hello-api`'s median goes from 0.82 ms to 0.42 ms with `cache` + `nolog`.

With all three off, what's left for `/hit` is §2.4. 64 connections write the same key, so runs
retry and then serialize on the cell (`firing.rs:167–300`, by design), and each commit makes two
fsyncs under the cell lock and the store lock.

## 4. The changes

Each change is laid out the same way: **what**, **today**, **after**, **touches** (every site,
with what happens there), **results**, **side effects**, **tests** and **docs**.

### C1. Closures by hash, in memory

**Status: built** (build step 2), as described here. The earlier draft kept one `MapResolver` per
flow and bounded the cache by units. Measuring the samples showed that bounded the wrong thing:
units are few and sizes vary by 1,000× (below).

**What.** Each closure stays in memory after it is first loaded, keyed by its root hash, so a run
looks its flow up instead of reading it from disk. Units are shared across flows, and the memory is
bounded in bytes by a server setting.

**Before.** `materialize_closure` built an owned `MapResolver` from scratch on every call
(`units.rs:159` at `aa2704a`), once per run, per request or per HTTP call. A hash missing from the
store was left out without an error ("A hash that doesn't resolve in the store is left out of the
resolver rather than erroring").

**What flows weigh.** Measured through `holon_author::agent::plan` over all 52 samples:

| | Units | Bytes |
|---|---|---|
| The median sample (`client-credentials`) | 34 | 5 KB |
| The largest (`chess`) | 88 | 6.5 MB |
| Samples with a js node (10 of 52) | | each places the pinned JavaScript engine module, 3.2 MB (`chess` two) |
| All 52, each flow holding its own units | 1,855 | 36.0 MB |
| All 52, each unit held once by its hash | 830 | 6.7 MB |

A unit count says nothing about memory here. The engine module is pinned, so every js flow places
the same unit, and holding it per flow would multiply it.

**As built** (`crates/holon/src/units.rs`):
- **`Loaded`** (`units.rs:156`) is a closure as runs resolve it: `Arc<BTreeMap<Hash, Arc<Unit>>>`. It's
  cheap to clone, and each unit is shared with every other closure that places it. It implements
  `Resolver` (`units.rs:158`), so every caller passes `&resolver` as before.
- **`closure(store, root)`** (`units.rs:185`) answers from the cache, or loads and keeps.
  `load_closure` (`units.rs:200`) walks as `materialize_closure` did, with the same bounds
  (`MAX_CLOSURE_UNITS`, `MAX_CLOSURE_DEPTH`). It takes each unit the cache already holds from memory
  and reads the rest from the store, which checks each against its hash. No cache lock is held while
  the store is read. It also says whether every reference resolved.
- **`FlowCache`** (`units.rs:243`) keeps closures by root hash, and their units by hash with a count
  of the closures holding each. Its budget is in bytes, each unit's encoded size counted once.
  - `keep` (`units.rs:300`) sizes a closure's units before taking its lock (encoding a 3.2 MB module
    takes a while). It doesn't keep a closure bigger than the whole budget.
  - `shrink` (`units.rs:335`) lets go of the least recently used closures until the cache fits, and
    a unit goes when its last closure does.
  - The lock is a leaf: nothing else is locked under it.
- **Only a whole closure is kept, and never an error.** A unit put later could complete a partial
  closure, and `TooBig` would fail the same way again.
- **No invalidation is needed.** A redeploy is a new hash. A rollback is a hash already seen. A
  resumed run evaluates the unit it started on (`durable.rs:633–637`), found by its own hash. Units
  are never swept (§2.1).
- **The cache lives on the store handle** as its fourth part (`main.rs:282`), beside `calls::Hub`
  and `sweep::Puts`, so every caller already holds it. A closure is valid for the store it came
  from, so a cache per store is the right scope.
- **The budget is a server setting, not a deploy setting.** The cache is shared: one closure serves
  every deployment, version and resumed run that uses it, and no deployment owns an entry. Letting
  go of an entry costs only speed (the next run reads the flow from the store, as every run did
  before), never correctness. So nothing a flow asks for or a person approves is involved.
  - The flag is `holon serve --flow-cache SIZE`, or `HOLON_FLOW_CACHE` (`main.rs:708, 734, 1033`).
    It takes `64M`, `1G` or `0` to keep none, parsed by `verify::budget_of` as `holon folder --cap`
    is.
  - The default is 256 MiB (`FLOW_CACHE_BYTES`, `units.rs:237`), about 40 times all the samples.
  - It's documented in `docs/manual/reference/cli.md`.
- **Callers.** All of them call `closure` in place of `materialize_closure`, which is gone:
  - per run: `listeners.rs:95`, `scheduler.rs:125`, `arrivals.rs:373`, `deploy.rs:924`,
    `deploy.rs:1079` and `durable.rs:637`;
  - per deploy or load: `deploy.rs:162` and `persist.rs:132`, which warm the cache for the first
    request;
  - the rest: `deploy.rs:507`, `deploy.rs:749`, `grants.rs:19`, `firing.rs:761` and `units.rs:256`;
  - the tests' four uses.

  None needed another change, because each passes the closure where `&impl Resolver` is taken.

**Results.** Measured with the throughput check (build step 1) on the same machine, 33 deployments:

| | Before | After C1 |
|---|---|---|
| `GET /hello/Ada`, 1 thread | 3,697 req/s, p50 192 µs | 7,383 req/s, p50 69 µs |
| `GET /hello/Ada`, 64 threads | 2,287 req/s | 6,734 req/s (2.9×) |
| `POST /calc`, 64 threads | 1,470 | 5,096 (3.5×) |
| `GET /hit` and `GET /last`, 64 threads | 74 and 80 | 86 and 94 (still bound by the deployments file, C3) |
| `hello`, live over HTTP with `wrk -c64` | 1,575–1,670 | 4,987 |

**Side effects.**
- **Memory**, up to the budget: all 52 samples at once would take 6.7 MB.
- **Integrity** (I-d). Each unit is checked against its hash when it's read from the store, and a
  kept closure is exactly what its hash names. A store file changed on disk afterwards is never
  read again; before, such a file failed its hash check and the run failed with a 500.
- **No behaviour changes.** No answer, log entry or run id changes, since evaluation reads the same
  units.

**Tests** (`tests.rs:7240–7300`):
- `a_closure_is_kept_by_its_hash_and_its_units_are_shared`: the same `Loaded` twice, one leaf held
  once by two closures, and the bytes counting it once;
- `a_partial_closure_is_loaded_again_once_it_is_whole`;
- `the_flow_cache_lets_go_of_the_least_used_past_its_budget`: the least recently used goes, the units
  only it held go with it, it loads again, and a budget of 0 keeps nothing while runs still get
  their closures.

Unchanged and passing: the closure-bound test
`a_closure_too_big_to_run_is_refused_and_every_sample_is_well_inside`, and the other 377 tests of
the crate.

**Docs.** The `holon serve` options and environment variables in `docs/manual/reference/cli.md`.

### C2. The run log off the table lock

**Status:** (a) and (b) built (build step 3): two deployments driven together went from 5,113 to
11,224 req/s combined. (c) built (build step 4): a cut-back of a real 2.7 MB log went from 14–25 ms
to 4.4–6.0 ms, and `hello` alone from 7,482–7,902 to 11,197 req/s at 64 threads (the plan has the
numbers). (d) built (build step 5): logs synced every second and before each witness send, at no
measurable cost to throughput. (e) built (build step 9a): a strict run answers once its entry is
on disk, and runs at once share the sync, about 4–5 runs a sync at 8 and 64 threads. `hello`
strict runs at 9,531 req/s at 64 threads on the throughput check (better than the 6,078 the
throwaway build measured) and `lazy` at 14,464; one at a time, 3,189 against 16,322 (p50 291 µs
against 50 µs). The watermark is a seq, not a byte offset (below), since a cut-back renumbers no
entry but rewrites every offset.

**What.** Each deployment's log gets its own lock, the line is appended through a file kept open,
and a cut-back copies bytes instead of parsing. A line still reaches the OS before its run
answers. A `strict` run (the default, C5) waits for it to reach the disk too (e), and a `lazy` one
lets the background syncer get it there within a second (d). Either way, no head reaches a witness
before its line is on disk.

**Today.** `Deployment.log: Log` (`main.rs:483`) is reached only through the table lock. `Log`
(`recorder.rs:354–375`) holds `next_seq`, `entries`, `keep`, `max_age_ms`, `file_oldest_ms`,
`file`, `written` (lines in the file), `cut`, `head`, `breaks` and `slug`. §2.2 describes what a
push does.

**No line is ever fsynced.** `keep_line` (`recorder.rs:521–549`) appends with a plain `write_all`,
and only a cut-back's `write_private` syncs (`sealing.rs:291`). The events log is the same: its
`keep_line` (`events.rs:150–170`) appends one event at a time and never syncs. Each head is queued
for the witness **before** its line is written: `Log::push` calls `self.export("run", …)`
(`recorder.rs:509`) and then `keep_line`, and `Events::push` calls `heads::note` (`events.rs:142`)
and then `keep_line` (`events.rs:145`). The exporter sends what's queued every 5 s (`main.rs:990`,
`Exporter::run`, `heads.rs:181–186`).

A process crash loses no line, because the OS still holds it. **A power loss or OS crash can
lose the newest lines**, and that has two consequences:

- **State can be ahead of the log.** Every state change is fsynced before its run answers (§2.4),
  but its line isn't. After a power loss, the state can reflect runs the log no longer has, and
  `holon verify`'s state check (`check_states`, `verify.rs:469`) reports "it began from a state the
  run before it didn't leave".
- **A witness can hold heads the disk lost.** A head sent in the 5 s before the power loss may name
  a line that never reached the disk. To the witness, the log then looks cut short, which is the
  tampering a witness exists to catch.

**After.**

- **(a) Its own lock.** `Deployment.log: Arc<LogCell>`, where
  `LogCell { log: Mutex<Log>, next_seq: AtomicU64 }`. `log_run` holds the table lock only to clone
  `Arc<LogCell>`, the secrets `Arc`, the channels `Arc` and the session number. It builds the
  `Redactor` and redacts the entry with **no lock held**. Then it locks that deployment's log to
  set `seq` and `prev`, compute packs, push and append, and it **returns the `seq` it assigned**.
  `next_seq` is mirrored into the atomic under the log lock, so a reader that only needs it never
  takes the log lock while holding the table (`events.rs:205`, `deploy.rs:1161`, `deploy.rs:1451`).
- **(b) A file kept open.** `Log` keeps `Option<File>` opened in append mode on the first write.
  `keep_line` becomes one `write_all`, with no `create_dir_all`, `exists` or `open` per line. The
  directory is created when the file is first opened. The line still reaches the OS before the
  answer (I-a), and nothing is buffered across runs.
- **(c) Cut-back without parsing.** `Log` keeps `lines: VecDeque<(u64 offset, u32 len, u64 at_ms)>`
  for each line in the file that parsed. `open_retaining` (`recorder.rs:409–448`) fills it from the
  read it already does, and `keep_line` appends to it. `cut_back` picks the kept lines from it,
  using the same rule as today (`recorder.rs:565–572`: the newest `keep`, none older than the age,
  always the newest line). It reads only those byte ranges from the file, writes them to the temp
  file, and renames that over the log (`write_private`). Lines that didn't parse (a line a crash cut
  short) are dropped at cut-back, as today, because only parsed lines are indexed. The handle is
  **closed before the rename and reopened after**. That's required on Unix, where the old handle
  would keep the replaced file, and it avoids renaming over an open file on Windows.
- **(d) Synced lazily, within a bound, for `lazy` handlers and for requests no route took.** For
  those runs the log stays off the request's critical path for durability, as it is today. It is
  fsynced in the background instead of never. (A `strict` handler, the default, syncs its own
  record before it answers: (e).)
  - **Dirty flags.** `Log` and `Events` each gain `dirty: bool`, set by `keep_line` when it appends
    and cleared when a sync takes it.
  - **`recorder::sync_logs(deployments) -> Result<(), String>`.** Under the table lock, it clones
    each deployment's `Arc<LogCell>`, and, for each deployment whose events log is dirty, takes that
    flag and file path (events are pushed under the table, `events.rs:203–206`). Then it releases
    the table. For each log it locks the log, takes the flag and path, and releases it. With no lock
    held, it opens each taken file and calls `sync_data`. On Linux an fsync through any descriptor
    flushes the file's data, so the append handle isn't shared. A failed sync puts its flag back
    and returns the error.
  - **Why taking the lock is enough.** A head is queued inside `push` while the push holds the log
    lock (or the table lock, for an event). So by the time `sync_logs` has taken a log's lock, every
    head already queued for that log has its line written, and the sync after covers it.
  - **Against a cut-back.** A sync that opens the path after a cut-back's rename flushes the new
    file, which `write_private` already synced, plus any lines appended since. One that opened the
    old file just before the rename flushes the old inode. The lines that file held were copied
    into the new file and synced there, and lines appended afterwards set the flag again.
  - **A syncer thread** calls `sync_logs` every `SYNC_EVERY` (1 s, §8 Q7), started beside the
    clock scheduler and the file-text sweeper (`main.rs:1075–1080`).
  - **Before every witness send.** `Exporter::flush` (`heads.rs:130–160`) calls a hook just before
    each batch's POST (`heads.rs:141`), after the batch is taken from `pending`. If the hook fails,
    flush returns `Err` and sends nothing, and the exporter's existing back-off retries
    (`heads.rs:181–200`). `heads::start` runs before the deployments table exists (`main.rs:990`
    against `main.rs:1074`), so the hook is installed afterwards with a new
    `heads::before_send(Box<dyn Fn() -> Result<(), String> + Send + Sync>)` that calls `sync_logs`.

  *Considered and rejected: writing the log in the background,* answering first and appending later
  from a queue:
  - **It gains little.** After (a)–(c), the log's 35–45 µs per run is mostly CPU (serializing and
    hashing), which a background thread still spends. On a CPU-bound host, throughput wouldn't
    move, and latency would drop by under 10% (the 35–45 µs out of about 0.42 ms at one
    connection, §3).
  - **It breaks I-a.** A process crash after the answer and before the write would leave runs that
    answered, called out and changed state with no record. Today a process crash loses nothing.
  - **Readers lose read-your-writes.** An agent reading its run right after the request finds no
    entry. `run_action` names an entry not yet written ("its run log says why (entry {seq})",
    `deploy.rs:1125`). Every reader would have to flush the queue first.
  - **Channel packs lose their timing.** They go out "as it is logged… what a subscriber gets is
    what the log holds" (`firing.rs:791–792`), so a queue would put them ahead of their record or
    delay them.
  - **The queue is no help under load.** An unbounded queue grows without limit when the writer
    falls behind, and a bounded one blocks, which makes logging synchronous exactly under load.
  - **A failed write surfaces too late,** after the caller has its answer.

  The latency it would buy is available without those costs. Redact and serialize the entry body
  before taking the log lock, and splice `prev` in under it, so only the hash and the `write` are
  serialized. Batched appends, where each run waits for its batch's `write`, are the fallback if
  the `write` still shows in a profile.
- **(e) Strict: the record on disk before the answer, with group commit.** This is the default level
  (C5).
  - **Waiting for the disk.** `log_run` returns the `seq` it assigned and the byte offset where its
    line ends. A strict run then calls `LogCell::sync_through(end)` before answering.
  - **Sharing one fsync.** `LogCell` gains `synced: Mutex<u64>`, the offset known to be on disk.
    `sync_through` locks it. If `synced >= end`, a sync that finished while this run waited already
    covered its line, so it returns at once. Otherwise it reads the current end of the file under
    the log lock, calls `sync_data` with no log lock held, and sets `synced` to that end. One fsync
    covers every line appended before it began, so concurrent runs of one deployment share it.
  - **Where it's called:** after `log_run` in each run path: `listeners.rs:226`, `scheduler.rs:160`,
    `arrivals.rs:407`, `deploy.rs:955`, `deploy.rs:1119` and `durable.rs:651`. Only a request has a
    caller waiting. For a clock tick, a file arrival or an action without a caller, "before the
    answer" means before the run counts as finished, which is before its persist and before it
    returns.
  - **Against a cut-back.** A cut-back rewrites the file through `write_private`, which syncs it,
    and then sets `synced` to the new file's length under the same lock, so offsets keep meaning
    the same file.
  - **Measured** with a throwaway build: the flow cache, the log off the table lock, and each
    request appending a 5 KB line to its deployment's file, then calling `sync_data` under exactly
    this group commit (Appendix A):

    | `hello-api` | Without | With strict |
    |---|---|---|
    | 1 connection | 3,448 req/s, p50 0.27 ms | 1,109 req/s, p50 0.86 ms (**+0.6 ms**) |
    | 64 connections | 21,507 req/s, p50 2.8 ms | 6,078 req/s, p50 10.4 ms (**−72%**) |

    `calc` at 64 connections went from 16,032 to 5,761, and `counter /last` from 12,771 to 5,749. An
    fsync of an **appended** file costs about 0.6 ms on this disk, against 113 µs for the small new
    files the state path writes (§2.4). An append changes the file's size, so the fsync must commit
    that metadata through the filesystem's journal too. Faster disks narrow the gap, and slow cloud
    disks widen it.
  - **What strict guarantees:** an answered run has its record on disk ("answered means recorded").
    It doesn't guarantee that a run which acted has a record. If power is lost after a run's
    outbound call and before its sync, the call happened and the record didn't survive. The caller
    got no answer, though, so a webhook sender (Stripe, say) retries, and the retry is recorded.
    "Acted means recorded" would need each outbound request recorded on disk before it is sent, as
    write-ahead for the round loop. That is not part of this note (§8 Q8).

**Touches.** Every use of a deployment's `log`:

| Site | Function | Today | After |
|---|---|---|---|
| `main.rs:483` | `Deployment` | `log: Log` | `log: Arc<LogCell>` |
| `firing.rs:786–799` | `log_run` | everything under the table lock | clone the `Arc`s, redact unlocked, push under the log lock; returns `seq` |
| `recorder.rs:497–516` | `Log::push` | serialize, hash, export, prune, `keep_line` | unchanged, except it records the line's offset |
| `recorder.rs:521–549` | `keep_line` | open per line | write through the open handle; record `(offset, len, at_ms)` |
| `recorder.rs:554–593` | `cut_back` | read, parse all, rewrite | copy the kept ranges, reopen the handle |
| `recorder.rs:409–448` | `open_retaining` | parses each line | also records offsets |
| `recorder.rs:452–468` | `retain` | reopens when keeping more | unchanged (reopening rebuilds the index) |
| `recorder.rs:611–616` | `lines` | reads the file | unchanged, called under the log lock |
| `recorder.rs:625–629` | `delete_file` | removes the file | closes the handle first |
| `deploy.rs:338–341` | `deploy` (redeploy) | `mem::take(&mut old.log)` then `retain` | clone the old `Arc<LogCell>`, lock it to `retain` |
| `deploy.rs:339`, `persist.rs:221–222` | new deployment, load | `Log::for_deployment` | wrapped in `LogCell` |
| `deploy.rs:841` | `handle_delete_deployment` | `delete_file` under the table lock | the same, through the log lock |
| `deploy.rs:1118–1119` | `run_action` | reads `next_seq`, then `log_run` in a second lock (Appendix B.1) | uses the `seq` `log_run` returns |
| `deploy.rs:1155–1165` | `handle_get_log` | `after`, `next_seq`, `head`, `breaks` under the table lock | clone the `Arc`, release the table, read under the log lock (one lock, so they agree) |
| `deploy.rs:1419`, `deploy.rs:1451` | `restore_plan` | clones every entry under the table lock | the same, under the log lock |
| `deploy.rs:1526` | `state_change` | the same | the same, under the log lock |
| `changes.rs:64` | `handle_get_changes` | the same | the same, under the log lock |
| `collections.rs:148` | `handle_get_collection` | `log.shows` under the table lock | under the log lock |
| `events.rs:205` | `Deployment::note` | `log.next_seq()` | the atomic |
| `events.rs:226` | `handle_get_events` | `log.next_seq()` | the atomic |
| `grants.rs:47` | `recheck` | `log.retain` | lock the log, then `retain` (`recheck` is called with `&mut Deployment` under the table; the log lock is taken inside, which is the allowed order, §5) |
| `verify.rs:100` | `handle_get_export` | `lines` and `head` under the table lock | both under one log lock, so the export's lines and head agree |
| `sweep.rs:137` | `named` | `log.lines()` for every deployment under the table lock | clone each `Arc` under the table, release it, read each log under its own lock |
| `folders.rs:1702` | `folders::sweep` | the same | the same |
| `tests.rs` | 3 direct uses of `.log.` | | updated to lock |
| `recorder.rs:354–375`, `521–549` | `Log`, `keep_line` (d) | no sync | `dirty` set on append |
| new, `recorder.rs` | `sync_logs` (d) | | takes the dirty flags and paths, then syncs with no lock held |
| `events.rs:150–170` | `Events::keep_line` (d) | no sync | `dirty` set on append |
| `heads.rs:50–58`, `130–160` | `Exporter`, `flush` (d) | sends what's queued | calls the `before_send` hook before each POST, and sends nothing if it fails |
| new, `heads.rs` | `before_send` (d) | | installs the hook once |
| `main.rs:1074–1090` | server start (d) | starts the scheduler, the arrival watcher and the text sweeper | also starts the syncer thread, and installs the hook when `--export-heads` is set |
| new, `recorder.rs` | `LogCell::sync_through` (e) | | group commit: one `sync_data` covers every line appended before it |
| `listeners.rs:226`, `scheduler.rs:160`, `arrivals.rs:407`, `deploy.rs:955`, `deploy.rs:1119`, `durable.rs:651` | each run path (e) | `log_run`, then answer or return | `log_run`, then `sync_through` when the handler is strict, then answer or return |

**Results.** *Measured*: removing the log's work from the table lock is worth 3,820 → 21,044 req/s
together with C1 (the `cache` + `nolog` row is the upper bound). *Estimated*: C2 keeps the log, so
about 35–45 µs per run remains under **that deployment's** lock: serialize 13–23, hash and
export a few, write about 5, and the cut-back's share once it copies instead of parsing (a 2–4 MB
copy every 500 runs, under 10 µs a run). That bounds one deployment near 20,000 runs a second,
with no bound across deployments. A cut-back stalls only its own deployment, for a few ms instead
of 33.

*(d), measured and estimated.* An fsync inside the server took 113 µs on average here, plus
31 µs for the rename (`strace -c` during `/hit`; 1,766 fsyncs for 882 requests). Syncing costs
one fsync per **dirty** log per second, whatever the request rate: about 0.015% of one core per
active deployment, on the syncer thread and off every request. Before a witness send, it adds at
most one fsync per log dirty since the last tick, about one per active deployment every 5 s. The
only thing a request does differently is set a flag. A sync holds a log's lock only to take the
flag and the path, not during the fsync. *Estimated*: no measurable change to §3's figures.

**Side effects.**
- **Lock order** gains one lock (§5). The log lock is never held while taking the table. A reader
  that holds the table clones the `Arc` and releases the table before locking the log, so a
  cut-back never stalls the table.
- **Run order within a deployment** is the order in which runs take its log lock, as today, where
  the table lock decides it. Runs of different deployments are no longer ordered against each
  other, and nothing depends on that: seqs and chains are per deployment.
- **One more open file per deployment** with a log, held for the server's life. 33 deployments
  is 33 descriptors.
- **Readers** (log page, export, changes) no longer block runs of other deployments while they
  clone a whole log (Appendix B.3).
- **File format, location and chain** are unchanged (I-b). `holon verify` and an operator's tail
  see the same bytes.
- **Witness export** still goes through `heads::note` (`heads.rs:106–120`), which opens and appends
  `pending.jsonl` under a server-wide lock on every run when `--export-heads` is set. That is
  unmeasured (Appendix B.2) and outside C2.
- **(d) bounds what a power loss loses.** Run and event records written at most about one
  `SYNC_EVERY` (plus an fsync) before the loss can still be lost. Nothing older than that can be.
  A head a witness holds always names a line on disk (I-f), because a head is only sent after
  its line is synced.
- **(d) doesn't close the state-ahead-of-log gap, only bounds it.** After a power loss, the state
  can still reflect up to about a second of runs the log lost, and `holon verify` reports that as a
  break at the end of the log. Closing it means syncing the record with the state head before the
  answer (§8 Q6).
- **(d) adds steady fsync traffic:** one fsync per active log per second. On a slow disk with 33
  busy deployments, that is up to about 66 ms of device time a second at 2 ms each, on the syncer
  thread. It competes for the device with the runs' own state fsyncs (C3), which could slow them
  slightly. Requests never wait for it.
- **(d) makes a witness send wait** for the sync, up to a few ms. A sync that fails (disk full, an
  I/O error) holds the witness back until a sync succeeds, which shows in the exporter's waiting
  count, and the server prints why.
- **(d) doesn't sync the witness's own queue file.** `pending.jsonl` (`heads.rs:114–120`) is still
  not synced, so a power loss can drop heads not yet sent. The witness then holds fewer heads, which
  is not a false alarm (§8 Q7).
- **(d) doesn't sync the directory** after a cut-back's rename, so that gap from §8 Q5 remains.

**Tests.** Unchanged and must pass, in `recorder.rs`:
`numbers_entries_reads_past_a_seq_and_keeps_the_newest` (710),
`each_entry_names_the_line_before_it_and_the_chain_survives_cutting_back` (733),
`a_line_changed_removed_or_inserted_outside_holon_is_a_break_at_the_entry_after_it` (765),
`entries_from_before_the_chain_read_back_and_the_chain_starts_after_them` (798),
`a_log_keeps_runs_by_age_and_reads_back_more_when_it_keeps_more` (835) and
`a_kept_log_reads_back_after_a_restart_skips_a_cut_line_and_stays_bounded` (877). In `tests.rs`:
`a_flow_log_survives_a_restart_and_goes_with_its_deployment` (3785),
`a_flow_log_is_a_chain_and_a_changed_run_shows_as_a_break` (3813),
`exported_runs_verify_and_a_rewritten_log_doesnt` (4174),
`a_flow_runs_another_flows_action_and_both_logs_say_so` (7419) and the secret-redaction tests
(4308, 4398, 4570, 4722, 6079).

New:
- a cut-back's file is byte-identical to today's for the same lines, including a cut line before
  it and a line with an unknown field;
- appends after a cut-back land in the new file;
- an action's reported `seq` is its own entry under concurrent runs (B.1);
- while a test holds deployment A's log lock, deployment B answers;
- (d) an append sets the dirty flag, `sync_logs` clears it, and a failing sync (a path that can't be
  opened) puts the flag back and returns the error;
- (d) a witness sees the hook run before every POST. Use the local witness server the `heads.rs`
  tests already start (`heads.rs:234`, `heads.rs:286`) and a hook that counts calls. A failing hook
  sends nothing, and the heads stay queued;
- (d) a line appended while a cut-back runs is synced by the next tick;
- (e) a strict run's answer comes after its sync. Count syncs with a test hook on `LogCell`, then
  send a request and check a sync covering its offset happened before the answer was returned;
- (e) concurrent strict runs share syncs: N runs in flight cause fewer than N syncs;
- (e) `synced` follows a cut-back (an append after it is synced by offset in the new file).

Losing power can't be simulated in a test. The tests pin the order the guarantee rests on
(written, then synced, then sent) and leave the disk's part to the OS.

**Docs.** For (d), `docs/manual/reference/server.md`'s run-log section (`server.md:163–176`) gains
"A run's entry is written before its answer and on disk within a second; a power loss can lose
the last second of entries." The witness paragraph (`server.md:180–182`) gains "A head is sent
only once its entry is on disk."

### C3. Persist what changed, and only that deployment

**Status:** (a) built (build step 6): a stateful run that only reads went from 79 to 14,150 req/s
at 64 threads, and from 53–66 to 10,382 live over HTTP. (b)–(e) built (build step 7): a stateful
run that writes went from 87 to 702 req/s at 64 threads, and from 74 to 986 live. A write now
writes only its head, and a run persists the deployments file only when it parks. (f) is still to
come, if measurements ask for it.

**What.** A run persists only when it changed something durable. A state change persists as a small
per-deployment pointer, not as the whole table.

**Today.** §2.3. The deployments file holds each deployment's state as `state_refs`, a key → value
hash map (`wire.rs:224–226`), and load rebuilds the state from it (`persist.rs:172–188` →
`State::load`, `holon-host/src/state.rs:111`). A committed run's snapshot is in the store already:
`save_snapshot` puts each changed value and a record of key → value hash, and the run logs that
record's hash as `state_after` (`firing.rs:103`, `firing.rs:230/298`).

**After.**

- **(a) Persist on change.** The per-run calls (`listeners.rs:228`, `scheduler.rs:197`,
  `arrivals.rs:412`, `deploy.rs:958`, `deploy.rs:1121`) persist only when **the run changed its
  state** or **parked**. "Changed" means `state_after` differs from the snapshot hash of
  `start_state` (`firing.rs:89`, hashed with `restore::snapshot_hash`, `restore.rs:36`, in memory).
  A run that wrote a value equal to the old one changed nothing and persists nothing. A resumed run
  always changes the parked set, so `durable.rs:684` stays unconditional.
- **(b) A state head per deployment.** `deployments.state/<slug>` (beside `deployments.logs/`,
  derived from the deployments path as `recorder::file_for` does, `recorder.rs:384`) holds the hex
  hash of the deployment's current snapshot. It is written with `write_private` (temp, fsync,
  rename):
  - by a run, right after `snapshot_after`, **still under the cell lock** (`firing.rs:230`,
    `firing.rs:298`). The cell gives the order: two commits write their heads in commit order, so a
    later head never gets overwritten by an earlier one. The values and snapshot record are durable
    (`write_atomically`) before the head names them;
  - by the two places that replace state outside a run, both under the cell lock:
    `put_state_back` (`deploy.rs:1613–1625`, restore) and the lifecycle's run-from-session
    (`deploy.rs:1709`). Both already save a snapshot of what they replace;
  - by deploy, for a new stateful deployment's empty state (`deploy.rs:382`).

  The cell needs to know where its head goes, so `StateCell` (`claims.rs:26–35`) gains
  `head: Option<PathBuf>`, set where cells are made (`deploy.rs:382`, `persist.rs:182–186`), and a
  method `write_head(&self, live: &State, snapshot: &str)`.
- **(c) The deployments file holds definitions.** `persist_deployments` is still called on every
  definition change: deploy (`deploy.rs:420`), delete (`845`), restore (`1361`), lifecycle (`1735`),
  grant (`grants.rs:126`), policy reload (`grants.rs:167`), and parked-set changes (a)/(b). It no
  longer carries the state at all: the record's `state_refs` (`wire.rs:224–226`) goes, and so does
  the loop that saves every deployment's state on each persist (`persist.rs:65–81`). That makes a
  definition change cheaper too.
- **(d) Load.** In `load_deployments`, the state comes from `deployments.state/<slug>`, through
  `restore::snapshot_refs(head)` (`restore.rs:46–57`) → `State::load`. No head means an empty state.
  A head naming a snapshot the store lacks is an error, as a bad `state_refs` is today
  (`persist.rs:180`).
  - **No migration.** Holon is in development, so a data folder from before this change isn't read
    forward. Its stateful deployments start empty, which a developer can reset by redeploying. The
    two old forms load reads today are removed rather than extended: `state_refs`, and inline values
    before them, for the live state and for sessions (`persist.rs:172–196`, with `migrate_inline`).
    Other historical readers that this change doesn't touch (records from before grants,
    `persist.rs:162–165`; versions from before rc.30) are left for a cleanup of their own.
- **(e) Delete.** `handle_delete_deployment` (`deploy.rs:841–845`) removes the head file, as it
  removes the log (`dep.log.delete_file()`).
- **(f) Later: the log line as the state commit.** With `strict` the default (C5), a run that writes
  state waits for two durable writes: its head under the cell lock (b), and its log line before the
  answer (C2 e). The log line already carries the snapshot's hash (`state_after`, `firing.rs:103`),
  so it could be the commit record. The head file would go, and a writing run would make **one**
  fsync, shared with the other runs committing at that moment.
  - **The obstacle is order.** Today a run's line is appended after its commit, under a different
    lock, so two runs can commit as A then B and log as B then A. `verify` allows for that already
    (`verify.rs:464–468`, runs "beside" each other). But if the log is the commit, B's line could
    reach the disk before A's. After a power loss, the state loaded from B's line would include A's
    change while A's record is lost.
  - **The fix is a commit ticket.** A run takes a ticket per deployment under the cell lock as it
    commits, and appends its line in ticket order: a run whose ticket is next appends; others wait
    for their turn. A sync then covers a prefix of commits, never a commit without the ones before
    it. Load takes the state from the newest line on disk that carries one.
  - **Not in the first build.** The ticket changes the run path, and (b) is simpler and correct. Do
    this step once the throughput check shows writing runs bound by the second fsync (§6 step 10).

**Touches.**

| Site | Function | Today | After |
|---|---|---|---|
| `listeners.rs:227–229` | `serve_flow` | `state.is_some() \|\| parked_any` | `changed \|\| parked_any` |
| `scheduler.rs:196–198` | `fire_one_clock_tick` | `is_state \|\| parked_any` | `changed \|\| parked_any` |
| `arrivals.rs:411–413` | `fire_one_arrival` | `state.is_some() \|\| parked_any` | `changed \|\| parked_any` |
| `deploy.rs:957–959` | `handle_post_trigger` | the same | the same |
| `deploy.rs:1120–1122` | `run_action` | the same | the same |
| `durable.rs:684` | `resume_parked` | always | always (the parked set changed) |
| `firing.rs:67–104` | `FiredRun` | `state_after`, `start_state` | a `changed()` helper |
| `firing.rs:230`, `firing.rs:298` | `fire_with_state` (commit) | `snapshot_after` | then `cell.write_head(...)`, under the same lock |
| `firing.rs:458–462` | `snapshot_after` | | unchanged |
| `claims.rs:26–35` | `StateCell` | `state`, `claims` | adds `head` and `write_head` |
| `deploy.rs:382` | `deploy` | makes the cell | gives it its head path, writes the empty head |
| `deploy.rs:1613–1625` | `put_state_back` | replaces live state | also writes the head |
| `deploy.rs:1707–1710` | `handle_lifecycle` (run from a session) | replaces live state | also writes the head |
| `deploy.rs:841–845` | `handle_delete_deployment` | deletes the log | also deletes the head |
| `persist.rs:172–188` | `load_deployments` | state from `state_refs`, or inline values | from the head; the two old readers removed |
| `persist.rs:22–92` | `persist_deployments` | saves every state, writes `state_refs` | writes definitions only; called less often |
| `docs/manual/reference/server.md:22–33` | data-folder table | | adds `deployments.state/`, "each stateful deployment's current state, as a snapshot hash" |

`sweep.rs` needs nothing. The sweep keeps whatever the **in-memory** live state names
(`sweep.rs:148–153`), not what the file names. A snapshot a head names is the live state's, so
it's kept.

**Results.** *Measured* (the `nopersist` switch, an upper bound for (a)+(b)): `/last` 53 → 879
req/s alone and 14,691 with C1 and the log off the table lock; `/hit` 65 → 863 alone and 1,453
with both. *Estimated*: a reading run makes no write at all, so `/last` should match the
measured 14,691. A writing run adds one small fsync'd file under its cell lock, so `/hit` should be
somewhat below 1,453. Neither depends on how many deployments the server has any more.

**Side effects.**
- **Counters persist less often.** `trigger_count`, `error_count` and the session tallies
  (`record_trigger`, `deploy.rs:867–875`) are no longer written on a read-only stateful run. They
  persist with the next write or definition change, as they already do for stateless deployments.
  A crash loses the counts since then and nothing else (§8 Q1).
- **State from before this change isn't kept.** A data folder written by an earlier build loads its
  deployments with empty state (load, (d)). There is no migration, since Holon is in development.
- **One more fsync under the cell lock** per writing run. Today the whole-table fsync happens
  outside the cell lock but under the global `PERSIST` lock. Runs of one deployment were already
  serialized by both, and runs of different deployments no longer wait for each other.
- **A new folder**, `deployments.state/`, with one small file per stateful deployment. Moving a
  server by copying the data folder (`server.md:38–39`) carries it. `holon migrate`
  (`migrate.rs:31–42`) concerns pre-rc.17 names only and needs no entry.
- **Crash between the snapshot and the head:** the head names the previous snapshot, and the run
  never answered, so nothing acknowledged is lost (I-c). **Crash after the head:** the new state
  loads.

**Tests.**

| Test | Effect |
|---|---|
| `state_is_kept_by_reference_and_its_root_survives_a_restart` (`tests.rs:2059`) | asserts `state_refs` in the file right after a write and across sessions (`tests.rs:2081–2084`, `2103`). It changes to assert the head names a snapshot with those references. The restart half is unchanged |
| `inline_state_from_older_files_moves_into_the_store` (`tests.rs:2111`) | removed with the inline-values reader it covers (no migration) |
| `trigger_with_state_persists_across_calls` (2242) | unchanged |
| `clock_tick_with_state_persists_across_ticks` (3169) | unchanged |
| `state_persists_across_a_simulated_restart` (3285) | unchanged; it now exercises the head |
| `unloadable_records_are_an_error_and_state_always_survives_a_restart` (2018) | unchanged |
| `persist_writes_whole_files_and_leaves_no_temp_files` (2173) | unchanged; extend it to `deployments.state/` |
| `a_collection_keeps_items_by_key_across_runs_and_restarts` (2474) | unchanged |
| `deployments_persist_across_a_simulated_restart` (1800) | unchanged |

New:
- a stateful run that only reads leaves `deployments.json` byte for byte unchanged;
- a write survives a restart through its head;
- a restore and a run-from-session survive a restart;
- deleting a deployment removes its head;
- concurrent writing runs leave the head naming the last commit.

**Docs.** `docs/manual/reference/server.md`'s data-folder table.

### C4. The store lock (after C1–C3 are measured)

**What.** On-disk store reads and writes stop serializing on one mutex, while the sweep keeps its
guarantee.

**Today.** `SharedStore` is `Arc<Mutex<DynStore>>`, with `DynStore = Box<dyn Store + Send>`
(`main.rs:272–310`). Every `get`, `put`, `get_value` and `put_value` takes the mutex, including
`FileStore::put_value`'s fsync. The sweep **depends** on that mutex. It holds `store.0` while it
checks whether a value was put lately and removes it (`sweep.rs:198–212`), and `Puts::put` is
"called with the store held, so a sweep, which holds it too while it removes a value, sees every
put before it or none of it" (`sweep.rs:64–66`). Without it, a run could find a value's file
present and skip writing it (`holon-host/src/lib.rs:677`) just before the sweep removes it.
`folders::kept_text` also locks `store.0` directly (`folders.rs:1683`).

**After.**
- `SharedStore` holds `Arc<RwLock<()>>`, a **sweep gate**, beside the backend. Puts and gets take
  it shared. The sweep's check-and-remove (`sweep.rs:199`) takes it exclusively, which keeps the
  guarantee above.
- The backend is called without an outer mutex when it is a `FileStore`, whose operations are safe
  side by side: names are content hashes, temp names are unique per call
  (`holon-host/src/lib.rs:643–646`), the rename is atomic, and two puts of one value write the same
  bytes. `Store::put` and `put_value` take `&mut self` (`holon-host/src/lib.rs:512–523`), so the
  backend becomes an enum: `Disk(FileStore)` called through `&self` helpers, and
  `Locked(Mutex<DynStore>)` for `MemStore` and the tests' own backends (`tests.rs:1807`,
  `tests.rs:3289`).
- `folders.rs:1683` goes through `SharedStore::get_value` instead of `store.0`.

**Touches.** `main.rs:272–310` (`SharedStore`, its `Store` impl), `main.rs:1014–1020` (where it is
built), `sweep.rs:164–212` (`sweep`, `store.2`), `folders.rs:1683` (`kept_text`), and
`holon-host/src/lib.rs:614–690` (`FileStore`: `&self` variants of `put` and `put_value`).

**Results.** *Not measured.* It removes the last server-wide lock from a writing run (§2.4). Its
worth shows on `/hit` with several stateful deployments written at once, which §3 didn't measure.
That's why it comes after C1–C3 are re-measured.

**Side effects.** The sweep waits for in-flight puts to finish, and puts wait for one removal at a
time. That ordering is the same as today's. Reading `sweep.rs` in full (`named`, `Puts` forgetting
at `FORGET_AT`) is part of this step, so the guarantee is checked rather than assumed.

**Tests.** Unchanged and must pass: the sweep tests in `sweep.rs` and `tests.rs`. New: a put racing
a sweep never loses a value a committed run names.

**Not proposed: group commit.** A writing run makes three small fsyncs (the value, the snapshot and
the head), one after another per deployment. On a slow disk (0.5–2 ms per fsync is common) that
caps one deployment at a few hundred writes a second, still several times today's 65. Moving those
writes out of the cell lock is subtle. Run B commits after run A and names A's values, so B's head
must not become durable before A's values are, and B's `save_snapshot` writes only what changed
since **B's** start (`restore.rs:25–28`). A per-server journal with one fsync per batch is the known
answer. It gets its own note if measurements on real disks ask for it (§8 Q4).

### C5. A log level per handler: strict by default, `lazy`, `unrecorded`

**Status: built.** The language and core side in build step 8: the words parse, lower to the
trigger node's `log` param (frozen in the handler's unit only when it isn't strict, so a flow
without them hashes as before), print back, and are checked in core (`may_record_lightly`, also in
`read_server`) and by `holon check` and the editor at the node that breaks the rule. The server's
side in build step 9: strict and `lazy` at run time in 9a, and `unrecorded` in 9b (approved per
handler, counted in a tally, failures logged). `GET /health` unrecorded answers 20,830 req/s live
at 64 connections, against 8,306 strict, and the measured 21,044 in the table below.

**What.** Every handler (route, clock, file handler, action) records its runs `strict`ly by default:
the record is on disk before the answer (C2 e). A handler that only reads can say otherwise, on its
own line:

```
route GET /health (req) unrecorded { … }   # no record; a summary row, failures counted
route GET /notes (req) lazy { … }          # recorded; on disk within about a second
route POST /notes (req) { … }              # strict: on disk before the answer
every 1m lazy { … }
action report (x) unrecorded { … }
```

| | `strict` (default) | `lazy` | `unrecorded` |
|---|---|---|---|
| For | anything that matters | speed on handlers that only read | log management: noise, volume, privacy |
| The record | on disk before the answer (C2 e) | to the OS before the answer, on disk within about 1 s (C2 d) | none. Failures are still recorded, lazily |
| In the runs table | each run | each run | one summary row per handler: runs, failures, mean duration, last run |
| Replay, `verify`, history, restore | yes | yes | no, except a failure's record |
| Allowed on | every handler | a handler that only reads (below) | a handler that only reads, approved, where the host's policy allows |
| Who agrees | nobody | nobody | a person, at approval (an exception item) |

**Why three levels.** These are the measurements, for `hello-api` at 64 connections (C2 e, §3):

| Level | Throughput | Added per request, 1 connection | Status |
|---|---|---|---|
| strict | 6,078 req/s | about 0.6 ms waiting for the disk | measured |
| lazy | about 15,000–16,000 req/s | about 40 µs of CPU | estimated |
| unrecorded | 21,044 req/s | none | measured, as the `cache` + `nolog` row |

`lazy` is close to `unrecorded` and far from `strict`, so `lazy` is the speed opt-out. `unrecorded`
is for what `lazy` can't do, which is keeping runs out of the log:

- **Retention displacement.** A deployment keeps its newest `keep_runs`, 500 by default
  (`permissions.md:94`). A health check once a second pushes every other run out within about 8
  minutes, and even the 20,000 ceiling lasts about 5.5 hours.
- **Large answers.** A run's record keeps up to 64 KiB of its answer (`server.md:173`), so a page
  route logs its page on every request.
- **Privacy.** Some reads shouldn't leave a trace of who asked.

*Considered and rejected: per-handler retention* (keep 20 of `GET /health` and 500 of the rest). The
log is cut back only from the front, which "keeps its newest line, so the chain goes on"
(`server.md:170`; `cut_back`, `recorder.rs:565–572`). Keeping fewer of one handler means cutting
lines out of the middle, and every later line would then name a predecessor that's gone, a break
to `verify` at each cut. Separate logs per handler would avoid that, but they need a shared order
across files for the state checks. `unrecorded` keeps such runs out of the chain from the start, so
the chain, cut-back and `verify` are untouched.

**Which handlers may be `lazy` or `unrecorded`.** Only a handler that only reads, which the graph
decides, not the author. Core already derives a unit's capabilities, failing closed: an unknown
leaf or an unresolvable child saturates to every capability (`capabilities`, `kind.rs:595`). It also
splits them into those read from outside and those performed (`Capability::is_sourced`,
`kind.rs:206–212`). The rule:

> A handler may be `lazy` or `unrecorded` when **every capability of its unit is sourced, and none
> is `Await`**.

That excludes state, collection, file and database writes (`StateWrite`, `CollectionWrite`,
`FileWrite`, `DbWrite`), outbound requests (`HttpRequest`, `NetworkEgress`), channel sends
(`ChannelSend`), flow calls (`RunsAction`), `Spawn`, answering with a secret (`SecretAnswer`), and
waiting (`Await`). Each of these breaks something when its run isn't recorded:

| A handler that… | Breaks, without a record |
|---|---|
| writes state, collections, files or databases | `verify`'s state check ("it began from a state the run before it didn't leave", `verify.rs:469`, which accepts only deploy, restore and lifecycle events as unexplained moves, `verify.rs:449–451`), the state history page (`changes.rs:54`), restore points, and the sweep, which keeps only what recorded runs name (`sweep.rs:131–158`) |
| waits (`await`) | resuming and checking: a resumed part's first part is "not here" (`verify.rs:502`) |
| sends on a channel | the channel's packs, "rebuilt from the log at load" (`main.rs:485`) |
| calls another flow's action | the callee's record, which the caller's names ("logged there as run `by` the calling flow", `calls.rs:7–8`) |
| calls out | nothing inside Holon, but the record of something done to the world, and its replay (a replay answers outbound requests from the record) |

A handler that breaks the rule fails to compile, at the node that breaks it: "GET /health can't be
`unrecorded`: it writes state.hits (line 12)". It works the way the secret checks point at a node.
The check runs where the server is read (`read_server`) and in `holon check`, so the editor, the
CLI and deploy agree. Under `lazy`, the same rule keeps strict's guarantee for every handler whose
runs change anything.

The levels decide per handler, but one thing decides per run. A `strict` handler whose run
performed nothing (a `POST` that failed validation, say) still syncs. Strict is the handler's
promise, and a handler that wants the cheaper level for such runs can't have it: only handlers
that **can't** act may be `lazy`.

**A request no route took** (404, 405) ran nothing and performed nothing, so it is `lazy`. Making
it `strict` would let anyone on the network cause an fsync per request, and fill the log with
scanners' probes. The policy may make it `unrecorded` (§8 Q12).

**Today.**
- **Handlers carry their markers as ordinary frozen params** on the handler unit, read in one place:
  an endpoint has `declare = "endpoint"`, `method`, `path` and `resume`, and the clock, file and
  action handlers have `declare` and their own (`declare.rs:1–24`, `Route` at `declare.rs:41`,
  `Clock` 55, `FileHandler` 65, `Action` 76, `Server` 86, `read_server` 191).
- **The resume marker shows the path from text to unit.** It's parsed after `(req)` (`resumes`,
  `parse.rs:582–587`), put on the route's `request` node (`lower.rs:1059`), carried to the endpoint
  unit (`routes.rs:268`, `routes.rs:450`), and read by core (`declare.rs:253`). `fmt` prints it
  (`print.rs:732`; contextual words at `print.rs:31`).
- **Approval items** are derived capabilities plus exceptions (`manifest_items`, `manifest.rs:28–51`:
  `Limit(…)`, `PlainHttp(…)`, `PrivateAddresses`, `AgentRuns(…)`). An exception needs the policy
  to allow it at all (`allow_plain_http`, `allow_private_addresses`, `permissions.rs:394–414`;
  `permissions.md:44–52`).
- **Counts are per deployment:** `trigger_count` and `error_count` (`record_trigger`,
  `deploy.rs:867–875`), shown by the editor's deployment card (`editor/src/DeploymentCard.tsx`).
  The live log comes from `GET /deployments/<slug>/log` (`deploy.rs:1155`; `getLog`,
  `editor/src/server.ts:709`).

**After.**
- **Text.** The handler lines take an optional word before the block:
  - `route METHOD /path (req) [resumes name] [lazy | unrecorded] { … }`;
  - `every DURATION [lazy | unrecorded] { … }`;
  - `on file … (f) [lazy | unrecorded] { … }`;
  - `action name (x) [lazy | unrecorded] { … }`.

  The words are contextual, like `resumes`, and names everywhere else. `fmt` prints them, `strict`
  is never written because it's the default, and the grammar in `LANGUAGE.md` §5.1 gains them.
- **Unit.** The handler unit gains a frozen `log` param, `lazy` or `unrecorded`, absent for
  `strict`. For a route it's carried like `resume`: from the `request` node, through `routes.rs`.
  For clocks, file handlers and actions it's set where their `declare` is (`routes.rs:385–393`, and
  its siblings for `on_file` and `action`). It's part of the hash, so changing a level is a new
  version, and the redeploy approval shows the difference.
- **Core.** `Route`, `Clock`, `FileHandler` and `Action` gain `log: LogLevel`. `read_server` reads it
  and fails on any other value (`DeclError`). A new `holon_core::log_level_allowed(handler, resolver)`
  applies the rule above, using `capabilities`. A new `first_performing_node` (in `holon-author`,
  where nodes keep their spans) finds the node to point at.
- **Approval.** `manifest_items` adds `Unrecorded("GET /health")` (the handler's label, as the log
  names it) for each unrecorded handler. It's shown as "don't record runs of GET /health (failures
  are recorded)". `lazy` asks nobody, because it's allowed only where nothing is done.
- **Policy.** `allow_unrecorded`, beside `allow_plain_http`. Without it, a deploy asking for
  `Unrecorded(…)` is refused, as one asking for plain HTTP is. Its default is §8 Q9.
- **Enforcement.** The permit decides per run (`Permit::allows`). If `Unrecorded(label)` isn't
  granted (revoked while running, or never approved), the handler's runs are recorded `strict`ly,
  the same rule as any narrowed grant. An agent can deploy the word but can't approve it, the
  existing rule.
- **The run paths.**
  - `strict` calls `sync_through` (C2 e).
  - `lazy` relies on the syncer (C2 d).
  - `unrecorded` skips `log_run` unless the run failed: an error, a 5xx answer, or a budget
    failure. A failed run is logged and synced lazily (§8 Q10). *Reversed since: an unrecorded
    run is never logged, failed or not; it is counted (resilience-audit-L4.md §2.8).*
  - Every level still counts (`record_trigger`).
- **Counts per handler.** `Deployment` gains `tallies: BTreeMap<String, Tally>`, with
  `Tally { runs, failed, total_ms, last_at }`, kept for unrecorded handlers, updated where
  `record_trigger` is, and persisted with the definition (C3 c), so a crash loses recent counts
  only. `GET /deployments/<slug>/log` adds `"tallies"`. The editor's live log and the terminal
  editor (`crates/holon/src/workbench`) show one row per unrecorded handler.
- **Agents.** The MCP tools that read a run say "runs of GET /health aren't recorded" for an
  unrecorded handler, instead of "no such run" (`crates/holon/src/mcp.rs`).

**Touches.**

| Site | What changes |
|---|---|
| `holon-author/src/lang/parse.rs:566–590` (`route`), `616–650` (`on_file`, `action`, `every`) | parse the optional word |
| `holon-author/src/lang/ast.rs:210` (`RouteDef`) and the `Every`, `OnFile`, `Action` defs | `log: Option<Name>` |
| `holon-author/src/lang/lower.rs:1059` | put `log` on the `request` node, like `resume` |
| `holon-author/src/routes.rs:268, 385–393, 450` | carry `log` to the handler units |
| `holon-author/src/lang/print.rs:31, 732` | print the word; add it to the contextual words |
| `holon-author/src/templates.rs:174` | the `request` template gains `log` |
| `holon-core/src/declare.rs:41–191` | `LogLevel`; the four structs; `read_server` |
| `holon-core/src/kind.rs` or `manifest.rs` | `log_level_allowed` |
| `holon-core/src/manifest.rs:28–51` | `Unrecorded(…)` items |
| `holon/src/permissions.rs:394–414` | `allow_unrecorded` in the policy |
| `holon/src/listeners.rs:207–226`, `scheduler.rs:160`, `arrivals.rs:407`, `deploy.rs:955, 1119`, `durable.rs:651` | choose by level: strict sync, lazy, or skip unless failed |
| `holon/src/deploy.rs:867–875` (`record_trigger`) | tallies per handler |
| `holon/src/main.rs:433–500` (`Deployment`) | `tallies` |
| `holon/src/persist.rs`, `wire.rs:220` (`DeploymentRecord`) | persist the tallies |
| `holon/src/deploy.rs:1155–1165` (`handle_get_log`) | `"tallies"` |
| `holon/src/mcp.rs` | the "not recorded" answer |
| `editor/src/server.ts:709`, the live log (`useViewMode.tsx`, `ViewPanes.tsx`), `DeploymentCard.tsx`, `Manifest.tsx` | summary rows; the approval item |
| `holon/src/workbench` | summary rows in the terminal editor |
| `docs/LANGUAGE.md` §5.1 and §5.11, `docs/manual/language/reference.md` | the words |
| `docs/manual/reference/permissions.md:44–52` | the `Unrecorded` exception and `allow_unrecorded` |
| `docs/manual/reference/server.md:161, 163–176` | "Every request is a run in the log" becomes "Every run is recorded, on disk before its answer, unless its handler is `lazy` or an approved `unrecorded`" |
| `README.md:3, 19` | the headline (§8 Q11) |
| a new sample, `crates/holon-author/samples/`, and its entry in `samples.rs` | §7 |

**Results.**
- **Measured:** `strict`, 6,078 req/s; `unrecorded`, 21,044 req/s (`hello-api`, 64 connections).
- **Estimated:** `lazy`, 15,000–16,000 req/s.
- **Unchanged, by construction:** a handler that writes is `strict` whatever its author says, so
  every run that changes anything is on disk before it answers.
- **What it saves for volume:** an unrecorded health check at one per second removes about 86,400
  records a day from its deployment's log, and the summary row shows the same traffic.

**Side effects.**
- **The default gets slower for read-only routes.** Without `lazy`, a read-heavy API runs at strict's
  pace (about a third of lazy's here). That is the cost of "recorded on disk before the answer" by
  default, and the reason `lazy` exists. The samples' read routes (§7) show the word where speed
  matters.
- **The headline changes** (§8 Q11).
- **One more exception** in the policy and the approval list.
- **Gaps in time, not in the chain.** Unrecorded runs never get a sequence number. A gap in a log's
  times is traffic that wasn't recorded, and the summary row accounts for it.
- **A failure's record without its neighbours.** A failing unrecorded run is recorded alone, so
  replaying it works (it only reads), but the runs around it aren't there to compare.
- **Static versus per run.** The level is the handler's. A `strict` handler pays for runs that did
  nothing, and nothing makes an effect-capable handler cheaper by declaration.

**Tests.**
- **`holon-author`:** each word parses, lowers to the `log` param, and prints back (`fmt` round
  trip). A word on a handler that writes, waits, calls out or sends fails to compile at the
  offending node, with its line and column. `strict` isn't printed.
- **`holon-core`:** `read_server` reads each level and refuses an unknown one. `log_level_allowed`
  is false for each excluded capability, and false for a saturated (unknown) unit, which is the
  fail-closed case.
- **`holon`:**
  - an unrecorded handler's run has no entry, and its tally counts it;
  - its failure has an entry;
  - revoking `Unrecorded(…)` makes the next run recorded;
  - a policy without `allow_unrecorded` refuses the deploy;
  - an agent's deploy that asks for it stops for approval;
  - a 404 is lazy;
  - a lazy run's entry is on disk after the syncer's tick.
- **Editor:** the summary row (`editor/src/*.test.ts`), and the new sample in `samples.test.ts` and
  the e2e deploy (`editor/e2e/samples.e2e.ts`).

**Docs.** The rows of the touches table under `docs/`, and `README.md`.

### 4.5 Later: the per-run constant

C1–C5 remove the work that grows with the server and put the record where it belongs. What's
left is a constant per run, and it explains §1.2's gap to .NET. This section measures it, and
prices three ways of shrinking it: caching and trimming the host's work, compiling each flow into an
execution plan, and generating native code. None of the three is part of this note's build. They
come after it, in that order.

**Where an unrecorded request's time goes.** A throwaway build timed each section of one request,
for `hello-api` (17 nodes) at 1 connection with `wrk -t1 -c1`. The build had the flow cache (C1)
on, and no log and no persist, which stands in for `unrecorded`. It also evaluated every request a
second time with no observer, which is safe because evaluation is pure, to separate the graph's
cost from the per-node recording. That duplicate (30 µs) is taken out of the total below, about
158 µs.

| Part | Code | µs | Share |
|---|---|---|---|
| Moving the run onto a blocking thread (20) and back (21) | `spawn_blocking`, `dataplane.rs:295`; the shared runtime, `dataplane.rs:68–76` | 41 | 26% |
| Evaluating the graph | `holon_core::eval` with `NoObserver`, `eval.rs:107` | 30 | 19% |
| Run setup and bookkeeping: run locks, the nonce, permit, sourced values, rounds (17), and the run id (7) | `fire_with_state`, `firing.rs:120`; `run_attempt`, `firing.rs:496`; `run_id_of`, `firing.rs:711` | 24 | 15% |
| The data-plane wrapper, outside `serve_flow` | `serve_data`, `listeners.rs:15–64`: `allowed_origin` runs here (`listeners.rs:42`) and again in `answer` (`dataplane.rs:291`), each locking the table and cloning the list of allowed sites | 17 | 11% |
| The per-node timeline | `recorder::Timer`, `firing.rs:560`; `Timer::produced` converts each node's outputs and serializes them to JSON to measure their size (`recorder.rs:326–332`) | 13 | 8% |
| After the run: the entry, the park check, the secret checks | `listeners.rs:141–160` | 12 | 8% |
| HTTP: parsing, reading the body, building `DataRequest` | hyper; `answer`, `dataplane.rs:263–289` | ~9 | 6% |
| Routing and the request record | `data::handle` | 8 | 5% |
| Lookup, closure, counts, other | | ~4 | 2% |

`calc` (29 nodes) gave the same picture with a larger graph: about 40 µs of evaluation and 14 µs of
timeline. Separate runs of the build varied by up to about 30%, but the proportions held. .NET answers
the same request in 49 µs in all (§1.2).

**What each would save.** The figures below are estimates built on the table, with nothing built
yet:

| `hello-api`, unrecorded | p50 at 1 connection | CPU per request, 64 connections | Throughput, 64 connections |
|---|---|---|---|
| C1–C3 with C5's `unrecorded` (the stand-in build, measured) | ~158 µs | 120 µs | 27,474 req/s |
| (a) + caching and trimming the host's work | ~90–105 µs | ~70 µs | ~40,000–45,000 |
| (b) + an execution plan compiled per flow | ~75 µs | ~55 µs | ~50,000–55,000 |
| (c) + native code generation | ~65 µs | ~48 µs | ~60,000–65,000 |
| .NET 8, plain (§1.2, measured) | 49 µs | 20 µs | 116,612 |

- **(a) Caching and trimming the host's work: about −55 to −70 µs, the largest of the three.**
  - **Run read-only handlers without the thread hop: −30 to −40 µs, the largest single saving.**
    Every run moves to the blocking pool because a run may block: on state locks, on outbound calls
    (`ureq` is blocking), or on fsyncs. C5's rule (every capability sourced, none `Await`) already
    identifies the handlers that can't do any of that. Such a handler can run on the async worker
    itself, or under `tokio::task::block_in_place`, which keeps the worker but moves its other
    tasks off it. `strict` runs still wait for an fsync, so they keep the hop, or wait for the
    group-commit sync on the async side.
  - **Skip the timeline and the entry for unrecorded runs: about −20 µs.** C5 decides that before
    the run starts, so the evaluator can run with `NoObserver`.
  - **Cache per flow, by hash (C1's cache), what the wrapper and run setup rebuild per request:
    −5 to −15 µs.** That covers the deployment's allowed origins (checked once, not twice), the
    route table, and the parts of the setup that depend only on the flow and its grant.
  - **For recorded runs**, the timeline's per-node outputs could be dropped. Evaluation is pure, so
    replay and `holon verify` can recompute every node's value from what the run read. That would
    save the per-node JSON sizing too. It changes what the log holds and what the editor's replay
    reads, so it's a decision of its own (§8 Q16), not a trim.
- **(b) An execution plan compiled per flow: −15 to −24 µs.**
  - **What it is.** Each closure is lowered, once per hash and cached beside it (C1), into a flat
    plan: children resolved to indices, parameters parsed, each primitive bound to a function
    pointer, slots numbered, and the lazy choices kept as jumps (`choice_next`, `eval.rs`). Plans
    like this typically run 2–5× faster than a graph interpreter, taking evaluation from 30 µs to
    about 6–15 µs.
  - **Why it's safe.** It lives in `holon-core`, so it keeps core's four dependencies
    ("purity-by-dependency", `holon-core/Cargo.toml:21`), runs in the editor's wasm build (P5, one
    implementation), and needs no toolchain on the server. Its results are checked against the
    eager oracle (`eager.rs`, kept "as the oracle the lazy evaluator is checked against") and against
    today's `eval`, on every sample and the core tests.
- **(c) Native code generation: another −5 to −12 µs on a flow this small.** This means compiling
  each flow to machine code: Rust through `rustc` into a library, a JIT such as Cranelift, or wasm.
  Evaluation would drop to about 1–3 µs. Even then Holon stays about 1.3× .NET in latency and 2× in
  CPU, for the machinery it keeps on every run (the run id, the permit, the locks): the price of
  its guarantees, not of interpretation. And the costs are high:
  - **A second implementation of every primitive.** It must match the interpreter exactly,
    floating point included, or replay and `verify` stop being exact.
  - **Slower deploys,** and a toolchain or JIT on the server.
  - **Native code made from graphs that agents wrote**, running inside the server process.
  - **It reverses two choices the project made deliberately:** core's dependency limit, and an
    engine picked for "no JIT, no platform code paths" (`wasmi`, `holon-module/Cargo.toml:12–14`).

**Where compiling matters: flows that compute.** For a small handler, the evaluation is 19% of the
request and code generation buys little. For a flow that loops over data, it's nearly all of the
run. The repo's bench (`cargo bench -p holon-core --bench native_vs_composite`, Appendix A) gives
about 68 ns per element for `fold`, against 1.3 ns for the built-in `sum`:

| `fold` over 10,000 items | Time |
|---|---|
| Interpreter (measured per element, scaled) | ~680 µs |
| (b) Compiled plan (estimated) | ~150–350 µs |
| (c) Native code generation (estimated from `sum`) | ~15 µs |

There, code generation is 20–50× faster than the interpreter and the only option that matters. If
such flows become a real use case, (c) is worth its costs for them. Until then, the built-in
primitives (`sum` and its siblings) and js holons (`wasm-module-L4.md`) are where heavy computing
goes.

**Measured end to end, after build step 12.** A route that computes statistics over a list of
numbers and a record per item, six `map` and `fold` passes in the graph (`CRUNCH` in `tests.rs`,
run by hand with `HOLON_PROFILE=crunch`), against the same computation in the C# service of §1,
both answering the same numbers to the last digit. Live, `wrk`, two rounds:

| | Holon, unrecorded | Holon, `length` and `sum` native | Holon, lazy | C# |
|---|---|---|---|---|
| 100 items: p50 at 1 connection; CPU per request | 1.00 ms; 943–977 µs | 0.77 ms; 707–743 µs | 1.33–1.38 ms | 0.08 ms; 46–50 µs |
| 1,000 items | 8.6–8.8 ms; 8.7–9.1 ms | 6.6 ms; 6.6–7.0 ms | 9.5–9.8 ms | 0.15–0.17 ms; 102–106 µs |
| 5,000 items | 43.7–44.0 ms; 44–49 ms | 30.8–31.3 ms; 33–34 ms | 48.1 ms | 0.58–0.60 ms; 466–477 µs |

About 8.7 µs an item, 1.5 µs for each pass's call of its function: 12–20× C# at 100 items, 75–100×
at 5,000. The native leaves take a quarter off, as the `map`s calling a function per item remain.
Under callgrind (1,000 items), the arithmetic itself (`primitives::compute`) is 4.5% of the run.
The rest is what the interpreter does around each call of the function, once per item:
- ordering its graph again (`topo_order`), 15.6%;
- resolving it by its hash (`SourcedResolver::resolve`), 10.8%;
- allocating and freeing its slots, records and lists, about 40%;
- cloning and dropping values (`Value::clone`, `BTreeMap` copies), about 20%.

So (b) is no longer an estimate for these flows: what it removes is most of their time.

**Its first part, built: a composite prepared once for a `map` or `fold`.** `topo_order` there
turned out to be only the composite's cycle check, and a `map` already resolved its function once;
what each application repeated was the rest of `eval_composite`'s preparation of the function's
graph: resolving its children, the index of which output feeds which input, the roots its walk
starts from, the cycle check. All of it depends on the unit alone, so `Prepared` (eval.rs) holds
it, made once before the loop and run for every item; `eval_composite` prepares and runs as
before. Same answers to the last digit; the workspace's tests, the editor's (which replay through
the same evaluator in wasm) and a short `hello` run's time (20.8–24.0 µs a run, alternating
builds) unchanged. `profile_one_route crunch`: 888 → 495 µs a run at 100 items, 8,427 → 4,584 at
1,000, 45,944 → 23,765 at 5,000. Live, two rounds:

| p50 at 1 connection; CPU per request | before | prepared | C# |
|---|---|---|---|
| 100 items, all in the graph | 0.98–1.02 ms; 937–962 µs | 0.62–0.64 ms; 589–607 µs | 0.085 ms; 34–36 µs |
| 1,000 items | 8.4–8.7 ms; 8.7–8.8 ms | 5.0–5.1 ms; 5.0–5.1 ms | 0.15–0.16 ms; 101–105 µs |
| 5,000 items | 42–43 ms; 43–45 ms | 24.6 ms; 25.3 ms | 0.58–0.59 ms; 465–477 µs |
| 5,000 items, `length` and `sum` native | 30.5–30.8 ms | 18.3–18.5 ms | |

Throughput 1.6–1.75× at every size; against C#, 7.4× at 100 items (from 12×) and 42× at 5,000
(from 73×). What remains is allocating and copying values, and the walk itself: the rest of (b).

**Then the interpreter's own path** (interpreter-path-L4.md, steps 1–3: composites' shapes kept,
the record's double work gone, inputs lent instead of copied). Live, against C# in the same
rounds: 12,188–12,272 req/s at 100 items, 1,516–1,573 at 1,000, 312–319 at 5,000, twice what
`Prepared` gave; p50 at one connection 0.35 ms, 2.5 ms and 12.1–12.3 ms, **4.0–4.2×, 16× and 20×
C#** (from 7.4×, 32× and 42×), CPU per request 301–309 µs, 2.5–2.6 ms and 12.4–12.7 ms.
Then the first level of a compiled plan (interpreter-path-L4.md §4, step 5): 14,133–15,468, 1,999–2,074
and 416–417 req/s, p50 at one connection 0.29 ms, 2.0 ms and 9.5 ms: **3.5×, 13× and 16× C#**.
Its second level (the walk worked out ahead for a function a `map` or `fold` applies, leaves called by
opcode): 16,937–17,027, 2,276–2,357 and 469–472 req/s, p50 0.27 ms, 1.74 ms and 8.4 ms: **3.2×, 11×
and 14× C#**.

**Where none of it matters much:**
- **Strict runs** wait about 600 µs for the fsync (C2 e). Cutting 50–90 µs of CPU might add 10–25%
  throughput at 64 connections, where CPU counts too, but latency barely moves. They're already level
  with a C# service doing the same work (§1.3).
- **Runs that call out** wait 50–300 ms on the network.

**Order.** Do (a) first, starting with the thread hop, because it reuses C5's rule. Do (b) when a
profile shows evaluation dominating a real flow, and (c) only when compute-heavy flows are a use case.

## 5. Lock order, before and after

Today, at `aa2704a`:

| Path | Order taken |
|---|---|
| `log_run` | table → secrets (`Redactor::of`) → channels' `sent_waiting` → heads' `pending` (with `--export-heads`); **file I/O under the table** |
| `persist_deployments` | `PERSIST` → table → each secrets lock; then (table released) each cell → store |
| a run's commit | cell → store (fsync inside) |
| a last attempt that calls another flow | cell (caller) → table (`run_action`, `deploy.rs:1055`) → … |
| `put_state_back` | table → cell (`deploy.rs:1614, 1617`) |

The last two rows invert each other (Appendix B.4).

After C1–C3:

| Path | Order taken |
|---|---|
| `log_run` | table (clone `Arc`s, release) → secrets (release) → **log** → channels' `sent_waiting` → heads' `pending`; file I/O under the log only |
| log readers | table (clone `Arc`, release) → **log** |
| a run's commit | cell → store → head file I/O (no lock) |
| `closure` | closures cache (lookup or insert only); loading takes the store, never while holding the cache |
| `sync_logs` (C2 d) | table (clone `Arc`s, take the events' flags, release) → each **log** in turn (take the flag, release); the fsync with no lock held |
| a witness send (C2 d) | `pending` (take the batch, release) → `sync_logs` → POST with no lock held |

The rules: **the log lock is never held while taking the table or a cell**, and **the closure
cache's lock is a leaf**. A witness send holds `pending` only to take its batch, never while it
syncs, so the existing order (log → `pending`, inside `push`) can't invert. C2 and C3 add no edge
that could close a cycle. B.4 is untouched by them.

## 6. Build order

The implementation plan, step by step with the code each step changes, its tests, its checks and
the numbers its commit reports, is `request-path-plan.md`.

One commit per step, per `CLAUDE.md`. Each commit gives its before and after numbers from the
throughput check (step 1) and says the step was run live.

1. **The throughput check.** An `#[ignore]`d test in `tests.rs` that starts the server in process,
   deploys `hello-api`, `calc` and `counter` plus 30 filler deployments, fires requests from N
   threads, and prints req/s, p50 and p99 per route. It runs with
   `cargo test --release -p holon -- --ignored request_throughput`. It is not a CI gate, since CI
   timing is too noisy. The commit records today's numbers.
2. **C1**, closures by hash.
3. **C2 (a)(b)**, the log's own lock and the open handle, with B.1 fixed by the returned `seq`.
   Steps 2 and 3 together are where the server starts to scale (§2.5).
4. **C2 (c)**, the cut-back without parsing.
5. **C2 (d)**, the logs synced lazily and before every witness send, with the manual's two lines.
   It's a durability fix rather than a speed one, and the commit says so. Its throughput numbers
   should match step 4's.
6. **C3 (a)**, persist on change.
7. **C3 (b)–(e)**, the state head, load, delete, and the manual's line. There's no migration: an
   earlier data folder's stateful deployments start empty.
8. **C5 in the language and core:** the words, the `log` param, `read_server`, the rule and its
   compile error, `fmt`, and `LANGUAGE.md`. Every handler still behaves as today (lazy, step 5)
   until step 9, so this step changes no number.
9. **C2 (e) and C5 in the server:** strict by default with group commit, `lazy`, `unrecorded` with
   its approval item, policy, tallies, editor rows and MCP answer, the new sample (§7), and the
   manual and README lines. This is where read-only routes without a word slow to strict's pace,
   and the commit gives both numbers.
10. **C3 (f)**, the log line as the state commit, when step 9's numbers show writing runs bound by
   the second fsync.
11. **C4**, after re-measuring with several stateful deployments written at once, and reading
   `sweep.rs` in full.
12. **§4.5 (a)**, one commit per item: read-only handlers without the thread hop, unrecorded runs
   without the timeline, and the per-flow caches. (b) and (c) are not scheduled; §4.5 says when each
   would be.

Steps 2–6 change no file format. Step 7 adds `deployments.state/` and drops `state_refs`. Step 8
adds a param to handler units. Nothing is migrated: Holon is in development, so existing flows and
data folders aren't carried forward, and a flow whose hash changes is simply redeployed. Steps 8
and 9 are one feature split in two: core and language first, then the host.

## 7. Samples

C1–C4 change no feature, so per `CLAUDE.md` they add no sample. `hello-api`, `calc` and `counter` are
what the throughput check deploys and loads. Each step is run live before its commit (deployed,
then sent load), and the commit says so.

**C5 is a feature, so it ships with one.** `crates/holon-author/samples/status-board.holon`, with its
entry in `samples.rs` ("a log level per handler"). It shows all three levels in one flow:

```
flow status-board
port 8170
collection notes

# Answers the load balancer. Not recorded: one summary row, its failures counted.
route GET /health (req) unrecorded {
  respond "ok"
}

# Lists the notes. Recorded, on disk within a second.
route GET /notes (req) lazy {
  respond notes.list(limit: 50)
}

# Adds a note. Strict, the default: on disk before the answer.
route POST /notes (req) {
  notes[req.json.id] = req.json.text
  respond "saved"
}
```

Without the three words it compiles today, already in `fmt`'s form, as "status-board: 12 nodes, 11
wires … it may: read items of its collection "notes", put and delete items of its collection
"notes", receive requests". Port 8170 is used by no other sample. Under C5's rule, `GET /health`
(only `NetworkIngress`) and `GET /notes` (adds only `CollectionRead`) may take a word, and
`POST /notes` (`CollectionWrite`) may not. Its "try it" line: send `/health` a few times and see
one row with a count, add a note and see its run, then list the notes and see that run too. The sample's text is written as `fmt` writes it
(the test says so), built by `editor/src/samples.test.ts`, deployed by `editor/e2e/samples.e2e.ts`
(its approval asks for `Unrecorded("GET /health")`), and run live before its commit. `notes-api`
and `orders-api` get a `lazy` on their list routes in the same step, since their hashes needn't
stay the same.

The end-to-end tests (`editor/e2e`) and `editor/src/samples.test.ts` must stay green throughout.

## 8. Open questions

1. **Decided: counters** go on in memory and persist with the next change to the deployment's
   definition, as a stateless deployment's always did (build steps 6 and 7). A write keeps only its
   state head.
2. **Decided: the flow cache's bound** is a server setting in bytes, `holon serve --flow-cache SIZE`
   (default 256 MiB), with units shared across flows (C1).
3. **Decided: no migration.** Holon is in development. C3 drops `state_refs` and the inline-state
   reader instead of keeping them for old data folders or a downgrade, and no step keeps existing
   flows' hashes for their own sake.
4. **Fsync on real disks.** Measure C3's per-write fsyncs on the hardware of the first outside job
   before deciding on group commit.
5. **Directory fsync after rename.** `write_atomically` (`holon-host/src/lib.rs:642`) and
   `write_private` (`sealing.rs:274`) fsync the file but not its directory, so on some filesystems a
   crash can lose an acknowledged rename. This note neither introduces that nor closes it.
6. **Decided: strict by default, per handler (C5).** This was the question "a strict log, per
   deployment?". It's answered per handler instead, with `strict` the default, and its cost
   measured at about 0.6 ms per appended-file fsync on this disk, not the 150 µs first guessed
   (C2 e).
7. **The sync interval, and the witness's queue.** Should `SYNC_EVERY` be 1 s and fixed, or a
   setting? And should `pending.jsonl` be synced on the same tick? That would cost one more fsync a
   second while heads are queued, and a power loss would then drop no queued heads.
8. **"Acted means recorded."** Strict guarantees that an answered run is recorded. Recording each
   outbound request on disk before it is sent (write-ahead in the round loop) would also cover a run
   that called out and then lost power. Is that wanted, and for which calls? It costs one fsync per
   outbound request, small next to the request itself. It needs its own note.
9. **Decided: `allow_unrecorded` is on by default.** The other exceptions default to off
   (`permissions.rs:413–414`), but `Unrecorded` is allowed only on handlers that only read, and a
   person approves each one at deploy, so a second switch in the host's policy would add friction
   and no safety. A host can still turn it off.
10. **Decided: an unrecorded handler's failures are recorded,** lazily: a failure is what someone
    looks into. No form that drops them too (`unrecorded always`) until a flow asks for one.
    **Reversed** (resilience-audit-L4.md, §2.8): a handler declared unrecorded is never logged,
    failed or not, nor run again. Its author may have declared it so to keep its requests out of
    the log; and a failure can be forced from outside, so logging and re-running each one made a
    route declared cheap an amplifier. Its tally keeps the last failure's time, status and error.
11. **Decided: the headline** "keeps a record of every run" (`README.md:3`) becomes "keeps a record
    of every run, on disk before it answers, except handlers a person approved as unrecorded, which
    change nothing", in build step 9b, when unrecorded handlers exist.
12. **Decided: requests no route took are `lazy`.** A way to make them unrecorded (a policy
    setting, or a header such as `unrouted unrecorded`), so scanners' probes stay out of the log
    too, can come later.
13. **Outbound reads under `lazy`.** A handler that only fetches (`http.get`) is excluded by the rule,
    because `HttpRequest` is performed. Core already tells reads from outside apart
    (`reads_from_outside`, `manifest.rs:90`). Should a handler whose only performed capability is a
    GET be allowed `lazy`? Its replay still needs its record, which `lazy` keeps.
14. **A second axis, how much to keep.** A page route's record keeps up to 64 KiB of its answer.
    Something like `lazy without body` would keep the record but not the page. Leave it until a
    real flow asks, but choose the syntax so it can be added.
15. **Sampling.** Something like `unrecorded, 1 in 1000` would record an occasional run of a noisy
    handler, which is harmless because it only reads. Leave it until asked.
16. **Decided: per-node outputs stay in recorded runs.** Each entry's timeline holds every node's
    outputs (`Timer::produced`). Dropping them for runs that read no secret, and computing them
    again when a run is opened, was tried (build step 12.5, the plan has the numbers): about 50%
    more lazy runs a second, and the values computed again matched the kept ones exactly for every
    run that could drop them, across the server's tests and the editor's end-to-end suite. It was
    still dropped, because current behaviour can't be kept without workarounds: the log file and an
    export would no longer hold what each step gave; replay's "reproduces the run" check would
    compare one computation with another instead of with the record; a run would show what its
    steps compute now, under a later release, not what they gave then; and a run's values would
    depend on its units and kept file texts outliving it. The record of what happened stays a
    record.

## 9. Invariant check

- **I-a, the record before the answer (strengthened).** By default (`strict`), a run's entry is on
  disk before its answer (C2 e). A `lazy` run's entry reaches the OS before its answer, as every
  run's does today, and the disk within about a second. `serve_flow` logs at `listeners.rs:226`
  and answers after it, and C2 keeps that order, changing only the lock it holds and adding the
  sync.
- **I-g, nothing that changes anything goes unrecorded or lazy (new).** Only a handler whose every
  capability is sourced, and none `Await`, may be `lazy` or `unrecorded`. The graph decides this,
  failing closed, and it's checked at compile and when the server is read (C5).
- **I-h, every exception to recording is someone's decision (new).** `unrecorded` is an approval
  item, capped by the host's policy, revocable while the flow runs, and never approved by an agent.
  It's part of the flow's hash, so a change is a new version (C5).
- **I-b, the chain.** One deployment's entries are numbered and chained in one order, under one
  lock, as byte-exact lines. A C2 cut-back copies bytes, so a copy still checks. The file's place,
  name and format are unchanged (`server.md:165`).
- **I-c, an answered write survives a crash.** A state change is durable before its run answers:
  values and snapshot (fsync'd), then the head (fsync'd), under the cell lock (C3). A run that
  changes nothing has nothing to make durable.
- **I-d, a flow runs what its hash names.** Closures are checked against their hashes when loaded,
  cached by that hash, and only whole (C1).
- **I-e, redaction before anything is written.** The entry is redacted before it is serialized. The
  only change is that C2 redacts with no lock held, using the secrets cloned out under the brief
  table lock.
- **I-f, a witness's head is on disk (new).** A head is sent only after the line it names has been
  synced (C2 d). Today this doesn't hold (Appendix B.5).
- **A power loss loses no strict record, and at most about a second of lazy ones (new).** Those
  are the guarantees of C2 (e) and (d). Today there is no bound.
- **M0 / §7a.** No disk I/O under the table lock on any run path: the log leaves it (C2), and the
  whole-table persist leaves the per-run path except for definition changes (C3). C2 (d)'s sync
  takes no lock while it fsyncs.

## Appendix A. How it was measured

- **Machine:** a 4-vCPU Linux container (6.18 kernel), ext4 on a virtio disk. `wrk` ran on the same
  machine over loopback, so absolute numbers are low and the ratios are what matter.
- **Holon:** `cargo build --release -p holon` at `aa2704a`, then `holon serve` with a fresh data
  folder, and the samples deployed with `holon deploy <file> --approve`. The 33-deployment runs add
  30 copies of `calc` under other names and ports.
- **.NET:** .NET SDK 8.0.133 from Ubuntu 24.04's archive, `dotnet new web`, with the endpoints
  (`/hello/{name}`, `/calc`, and `/last`, which answers a static string) as minimal-API handlers on
  `WebApplication.CreateSlimBuilder`, logging cleared, built in Release.
  - **The recording counterpart (§1.3, §1.4)** is a middleware in the same program, chosen by
    `RECORD` (`none`, `plumb`, `build`, `lazy`, `strict`), with `ThreadPool.SetMinThreads(128, 128)`.
  - It builds the record with `Utf8JsonWriter` into a thread-local `ArrayBufferWriter`, and scans it
    with `Span.IndexOf` for a secret's bytes.
  - It appends under one lock: the `seq` and `prev` prefix, `SHA256.HashData`, and
    `RandomAccess.Write` at the tracked offset, on a handle from `File.OpenHandle(…, FileMode.Append)`.
  - A strict request calls `SyncThrough`: under a second lock, it skips if the synced offset already
    covers its line, else it calls `fdatasync` (P/Invoke to libc) and records the offset reached.
  - The program isn't in the repository. This description is enough to rebuild it.
- **CPU per request:** the server process's `utime + stime` (`/proc/<pid>/stat`) across one `wrk`
  run, divided by the requests `wrk` completed.
- **The per-run constant (§4.5)** was measured with a throwaway build, reverted afterwards. It had
  `Instant` timers around each section of `answer` (`dataplane.rs`), `serve_flow`, `fire_with_state`,
  `run_attempt` and `run_id_of`, summed into atomics and printed as averages every 3,000 requests.
  It also ran a second `holon_core::eval` with `NoObserver` on each request's endpoint and input, to
  separate the graph's cost from the timeline's. The run used `wrk -t1 -c1 -d8s`, with the flow
  cache on and no log and no persist.
- **The comparison of §1.2–§1.4** was measured after §3, on the same machine. Holon's figures
  differ from §3's by run-to-run noise (`hello` unrecorded 27,474 here against 21,044 there).
- **The re-run after build step 12** (§1): `75a59c7`, `holon serve` with a fresh data folder and
  nine deployments, `hello-api`, `calc` and `counter` as their samples are (strict) and a copy of
  each route `lazy` and one `unrecorded` (approved, `holon deploy --approve`); the C# program as
  before, `RECORD=none`, `lazy` and `strict`. Each row: a 2 s warm-up, then 6 s at 64 connections
  (throughput, p50, CPU per request). The p50s at one connection were measured apart, after a fresh
  start, a 5 s settle and 3 s between routes, 8 s each, two rounds: taken right after a
  64-connection run, Holon strict read 150–260 µs high, the disk still busy with what that run
  wrote.
- **n8n and Node-RED (§1.5):** Node.js 24.22.2 (n8n 2.x needs 24), `npm install n8n@2.46.4` and
  `node-red@5.0.9`.
  - n8n's install needs `--force`: `n8n-nodes-base` lists an arm64 musl canvas binary as a plain
    dependency. Only the sqlite3 and isolated-vm install scripts were run.
  - The workflows were imported with `n8n import:workflow` and published with
    `n8n publish:workflow`, then run by `n8n start` with `NODE_ENV=production`, `N8N_LOG_LEVEL=warn`
    and diagnostics off. Each server was restarted for every setting.
  - A webhook path with a parameter is served under the webhook's id
    (`/webhook/<id>/hello/:name`).
  - CPU is summed over every process of the server, n8n's task runner included. Each row: a 3 s
    warm-up, 8 s at 64 connections (`--timeout 10s`), then 6 s at one, two rounds.
  - The workflows and flows aren't in the repository: each is the two or three nodes §1.5 names.
- **Load:** `wrk -t2 -c64 -d6s --latency`, or `-t1 -c1` for one connection. `calc`'s body is
  `{"op":"mul","a":6,"b":7}`.
- **Attribution.** In a throwaway build, reverted afterwards:
  - timers in `log_run` (lock wait, redaction and packs, serialization, push) and in
    `keep_line`/`cut_back`, plus a timer around `materialize_closure`, each printed as an average
    every 5,000 runs;
  - `strace -c -f` on the server under load: 84% of syscall time in `futex`, about 20 `openat` per
    request;
  - the `HOLON_X=cache,nolog,nopersist` switches of §3. A first pass taken right after each restart
    read low, because the server was still loading; the table uses a 3 s settle.
  - for strict (C2 e), a fourth switch, `strict`. Each request appended a 5,000-byte line to its
    deployment's own file and called `sync_data` before answering, with group commit (a run skips
    its sync when one that began after its write has finished). `strace -c` during `/hit` counted
    the state path's fsyncs (1,766 for 882 requests, 113 µs each on average). `counter /hit` is
    left out of the strict comparison: every write creates two new fsync'd files, the value store
    had grown to 102,639 files (412 MB), and `/hit` varied from 500 to 4,400 req/s between identical
    runs with either setting.
- **Evaluator:** `cargo bench -p holon-core --bench native_vs_composite` gave
  `traversal/native_sum/4096` 5.2 µs, `traversal/fold_sum/4096` 280 µs and
  `wrapping_tax/composite_add` 0.64 µs.

## Appendix B. Found along the way

These are recorded here so they aren't lost. B.1 and B.5 are fixed by this note (C2), and B.2 by the
plan's step 6b; B.3 is fixed by C2 (a), and B.4 is not.

1. **An action can report another run's entry.** `run_action` reads `next_seq` under one table
   lock (`deploy.rs:1118`) and logs in another (`deploy.rs:1119`, `log_run`). A run of the same
   deployment that logs in between makes the action report its entry (`seq`, used in
   `deploy.rs:1123–1125` and in "its run log says why (entry {seq})"). C2's `log_run` returns the
   `seq` it assigned.
2. **The witness queue opens a file per run under a server-wide lock.** With `--export-heads`,
   `Exporter::note` (`heads.rs:106–112`) holds `pending` while `append` (`heads.rs:114–120`) calls
   `create_dir_all`, `open` and `write`, for every run of every deployment. Measured in build step 5:
   `hello` served 7,890 req/s with a witness against 9,362 without. Fixed in the plan's step 6b, the
   same remedy as C2 (b): the queue file is kept open, and reopened after the rewrite that follows a
   taken batch. The gap is now 4–13% (9,294–10,195 against 10,580–10,727).
3. **Log readers clone whole logs under the table lock.** `handle_get_changes` (`changes.rs:64`),
   `restore_plan` (`deploy.rs:1419`) and `state_change` (`deploy.rs:1526`) call
   `log.after(None, usize::MAX)`, which clones every kept entry (500 by default, each with its
   timeline), while every run of every deployment waits. C2 moves them to the log lock.
4. **A possible lock-order inversion.** A last attempt holds its cell (`firing.rs:270`) for the
   whole run, flow calls included (`calls.rs:86` → `run_action` → table, `deploy.rs:1055`).
   `put_state_back` holds the table (`deploy.rs:1614`) and then takes a cell (`deploy.rs:1617`),
   and `handle_lifecycle` does the same (`deploy.rs:1709`). A restore of deployment X while X's
   last attempt is calling another flow would deadlock. It hasn't been reproduced. The fix belongs in
   `put_state_back` and `handle_lifecycle`: clone the cell's `Arc` and release the table before
   locking the cell.
5. **The run and events logs are never fsynced, and heads reach the witness regardless.**
   `keep_line` in `recorder.rs:521–549` and `events.rs:150–170` never syncs, and heads are queued
   before their lines are written (`recorder.rs:509`, `events.rs:142`) and sent every 5 s. A power
   loss can lose recent lines while their state changes (fsynced, §2.4) survive, and while a witness
   holds their heads, which looks like a log cut short. C2 (d) bounds the first and closes the
   second.
