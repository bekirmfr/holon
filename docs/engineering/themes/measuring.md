# Measuring

How a cost is measured in Holon, and the traps that have given wrong numbers. Every change made
for speed shows what it gained, and every change made for safety shows what it cost, measured the
ways below ([Standards](../README.md#standards-these-docs-record)).

## 1. The machine

The numbers in these docs come from one machine unless they say otherwise: a 4-vCPU Linux container
(kernel 6.18), ext4 on a virtio disk, the load generator (`wrk`) on the same machine over loopback.
A number from another machine is not comparable with them; a ratio measured in the same rounds
(Holon against C#, a build against the one before) mostly is. For strict, record the drive as
well as the machine: its flush decides the number ([Performance](performance.md#limits)).

## 2. Instructions

For a change to the evaluator or the request's bookkeeping, instructions per run under callgrind:
stable to a fraction of a percent, where wall time on this machine varies by up to 30%.

```
HOLON_PROFILE=hello HOLON_PROFILE_N=2000 \
  valgrind --tool=callgrind target/release/deps/holon-<hash> --ignored profile_one_route
```

`profile_one_route` (`crates/holon/src/tests.rs`) deploys one route (`hello`, `notes` or `crunch`;
`HOLON_PROFILE_ITEMS` sets `crunch`'s size) and runs it `HOLON_PROFILE_N` times in process.

- **Count the difference of two runs**, not one run divided: run it at two counts (say 1,000 and
  2,000) and divide the difference in instructions by the difference in runs, so the deploy and
  the setup are left out. Step 1a was first counted with the setup in; it was recounted both ways
  the same (`02ad957`).
- Read inclusive and exclusive costs per function (`callgrind_annotate`) to see where a run's
  instructions go before choosing what to change.
- Instructions are not time: a change that saves instructions by waiting (a lock, a sync) can be
  slower. A change to the record or the I/O is measured live too.

## 3. Live

For anything a request does end to end, `wrk` against a release server:

```
wrk -t2 -c64 -d6s --latency http://127.0.0.1:<port>/<route>    # throughput, p50, p99
wrk -t1 -c1  -d6s --latency http://127.0.0.1:<port>/<route>    # one connection: latency
```

`holon load <slug>` (load.rs) does the same from the command line, a route at a time, and adds
the server's side: the runs recorded, the log's syncs (`GET /deployments/<slug>/log`'s `syncs`,
since the server started) and the runs counted unrecorded, read before and after each route. Its
client is a few lines of HTTP/1.1 on sockets kept open, its latencies a fixed-size histogram
(within 1.6%), so it costs the machine about what `wrk` does. The `stress-test` sample is three
routes doing the same work, strict, lazy and unrecorded. On the machine above, 16 connections,
10 s each, when it was written: strict 7,731 req/s (77,309 runs in 12,136 syncs, about 6
runs a sync), lazy 14,596 (9 syncs), unrecorded 49,110.

- **A/B in alternating rounds**: the build before, the build after, then both again; report both
  rounds as a range. The machine drifts between minutes; one round each is not a comparison.
- **Warm up**: 2 seconds of the same load before the 6 measured.
- **CPU per request**: the server's `utime + stime` (fields 14 and 15 of `/proc/<pid>/stat`, in
  clock ticks) before and after, divided by the requests completed. Compare it at 64 connections:
  .NET spins its thread pool at one, inflating its figure.
- **Isolate a cost with a control**: to see what a background task costs a route, measure the
  route before, during and after it, and the same with the task replaced by a wait (the control)
  and by a version of it that does part of the work (a dry run). The store sweep's cost was all in
  its removals, which only the dry run showed.
- **Against others in the same rounds**: the C# service (plain, and keeping the same record) and,
  once, n8n and Node-RED, each run and measured as Holon is, one after another.

The in-process throughput check, `request_throughput`, drives `serve_data` from 1, 8 and 64
threads beside 30 other deployments. It leaves out hyper and the pool's hop, so at 64
connections it reads higher than live. Its fillers carry their documents, so the deployments file
is a real 1.1 MB; without them it was 32 KB and stateful routes read six times too fast.

## 4. Memory

- **What a cache holds** against what it counts: `measure_cache_memory`, which reads the
  allocator's bytes in use (`mallinfo2`) before and after filling each cache. It found the caches
  holding 2.8 and 7.8 times their sizes (S1).
- **A process over time**: `VmRSS` and `VmHWM` from `/proc/<pid>/status`, sampled.
- **The allocator keeps what it is given back.** glibc's threshold for using `mmap` adapts upward,
  and its arenas don't shrink, so a server's resident memory rises for minutes and steps up after
  any burst of allocation, without a leak. To tell a leak from the allocator, pin it:
  `MALLOC_MMAP_THRESHOLD_=131072 MALLOC_ARENA_MAX=2`. The soak's rise from 311 to 660 MB was flat
  at 453 MB pinned (at about half the throughput: the pinned allocator is slower).

## 5. Soaks

A mixed load on one server for 20–25 minutes, watching what grows. Used to find what a short run
can't: work done every ten minutes, growth on disk, memory.

- **Data**: a copy of a data folder from measuring (thousands of notes, logs of up to 60 MB), so
  the server starts as a used one would. Copied with `cp -a`, keeping the files' times: values older
  than an hour are then a sweep's.
- **Load**, each a `wrk` for the whole run: unrecorded, lazy and strict `hello` and the `notes`
  listing at 8 connections, `crunch` and unrecorded `calc` at 4, strict writes of random notes at 1,
  a route that always fails at 2.
- **Sampled every 10 seconds**: resident memory and its high-water mark, threads, open files, the
  logs' folder, the deployments file, one log's size, free disk. Every minute, a one-connection
  probe of unrecorded `hello`: requests a second, p50, p99.
- **After**: the data folder's size and file count; the server's log for panics and errors; each
  load's errors.
- **Stop on low disk**: the script stops the load below 2 GB free.

The scripts that ran the soaks and the comparisons are not yet in the repository; until they are,
this section is how to rebuild them.

## 6. Traps

Each of these gave a wrong number once:

- **Right after a heavy run**: measuring strict routes right after a 64-connection run read them
  150–260 µs slow, the disk still busy. Start fresh, settle 5 seconds, and leave 3 seconds between
  routes. The first pass after a restart reads low; settle 3 seconds.
- **A tracer slows what it traces**: `strace -f` without its seccomp filter took the server to
  1,363 requests a second. Count syscalls in a run of their own.
- **A store that has grown**: `/hit` varied from 500 to 4,400 requests a second once the value
  store held 102,639 files; it was left out of the strict comparison.
- **An fsync's cost depends on what changed**: about 0.6 ms for a file appended to (its size goes
  through ext4's journal), 113 µs for a small new file. A guess of 150 µs for strict's cost was
  four times low (Q6).
- **A virtual disk's sync isn't the drive's**: this machine's virtio disk syncs an appended file
  in 0.3–0.8 ms, a Windows desktop's SSD in about 2 ms. A cloud VM's fsync is answered by the
  hypervisor's storage, so a strict number from one says little of bare metal.
- **WSL2 isn't Linux on the drive**: on the same Windows machine, strict ran at half the native
  rate (803 against 1,649 requests a second), each sync passing through Hyper-V to a `.vhdx` on
  NTFS. Its syncs did reach the drive (slower, not faster), so it is durable, but its numbers
  are WSL's, not Linux's.
- **A stall that isn't the code**: maxima of 60–150 ms showed in every durable way of syncing on
  the Windows machine and never in the one without the drive's flush: the drive or the system
  holding up a flush now and then. Report the p99 and the rounds' p50s, and treat one max as
  one sample.
- **The baseline moves**: plain .NET itself varied from 96,980 to 133,151 requests a second on
  this machine. Compare within rounds.
- **A test fixture that isn't the real thing**: the throughput check's deployments file (above).
- **The allocator**: memory that rises is not a leak until it rises pinned (above).
