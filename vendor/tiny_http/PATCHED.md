# tiny_http 0.12.0, patched for Holon

This folder is a copy of [tiny_http](https://github.com/tiny-http/tiny-http) **0.12.0** as
published on crates.io (git `212b1c45`), the HTTP server under rouille, which serves Holon's
control port (the API the editor and the commands use). The workspace's `Cargo.toml` points
every use of `tiny_http` at it (`[patch.crates-io]`), and it is a workspace member: `cargo test
--workspace` runs upstream's tests (`tests/`, as published) and Holon's (`src/util/task_pool.rs`,
`src/limits_tests.rs`).

Every change is marked `Holon (PATCHED.md)` in the code. To see them all against upstream:

    diff -r ~/.cargo/registry/src/*/tiny_http-0.12.0/src vendor/tiny_http/src

(where cargo's cache still holds the published crate; otherwise download it from crates.io),
or, while the history keeps it, against the commit that added the copy unchanged:

    git diff $(git log -1 --format=%h --grep='tiny_http 0.12.0 as published, unchanged') -- vendor/tiny_http

## What was changed, and why

1. **Lost connections** (`src/util/task_pool.rs`, `spawn`). Each connection is a task that holds
   a pool thread while the connection lives. `spawn` queued a task whenever a thread was counted
   as waiting, but a woken thread stays counted until it holds the pool's lock again, so a burst
   of connections (a page opening, its browser's sockets all at once) queued some for threads
   already told of others. Those waited, unread, until some other connection closed: minutes, or
   for ever. A page's reads went unanswered. Now a task is queued only while the waiting threads
   outnumber the tasks already queued; otherwise it gets a thread of its own. Upstream's `master`
   has the same code (checked 2026-11-18).
2. **An idle limit** (`src/limits.rs`, `src/client.rs`, `src/request.rs`, `src/connection.rs`).
   Every socket read waits at most `limits::IDLE` (60 s) at a time. A connection is idle once it
   is owed no answer and the limit has passed since its last answer ended (or since it was
   accepted); an idle connection that has sent nothing of a next request is closed without an
   answer, at its next read that waits out the limit, so between one and two limits after its
   last answer. While a handler works on an answer, however long, the connection is not idle,
   whether or not its request had a body. A read inside a request's head that waits past the
   limit is told 408 (upstream's answer, which it gave only where a timeout was set); one inside
   a body is an error to whoever reads the body. Bytes that keep coming, however slowly, are
   not cut by it (a request's head and small body are, at their deadline: change 6), and a
   response is never cut. The limit is past Holon's deadline for a body
   (`http::BODY_DEADLINE`), so a body that stops is told 408 by Holon first; main.rs asserts it.
3. **A bound on open connections** (`src/limits.rs`, `src/lib.rs`), in all (1,024) and from one
   address (512). Upstream accepted every socket and gave each a thread for as long as it was
   open. One past either bound is answered 503 at once and holds no thread. Its sending side is
   ended at once, and one thread for all refused connections reads off and throws away what
   their clients send until each client closes, or until 2 s after its refusal. At most 256
   refused connections are kept, counted where the accept thread hands one over: past them,
   the oldest is closed there at once. Nothing waits uncounted in between, and the thread reads
   at most 64 KiB from one connection in a pass, so a client that never stops sending holds up
   neither the others nor their deadlines. A client that sent its request before reading, as
   every client does, then reads the 503, rather than a reset. With both bounds, the sockets
   the server holds are at most 1,024 accepted and 256 refused, so its own sockets can't run the
   process out of handles. One address may hold every request Holon answers at once (`MAX_IN_FLIGHT`, 512),
   which a proxy Holon trusts carries for all its clients; main.rs asserts both bounds hold
   Holon's in-flight bound. A Unix socket's connections count as one address.
4. **`Connection: close`** (`src/response.rs`, `src/request.rs`, `src/client.rs`). Upstream
   dropped a `Connection` header the application set. Now `Connection: close` is sent, and the
   response is the connection's last: no request after it is read and the client is sent the
   end of the stream. Holon's 408 for a body that stops relies on it.
5. **An accept that fails** (`src/lib.rs`, the accept thread; `src/limits.rs`, `Failing`).
   Upstream's accept thread ended at the first error from `accept`: it queued the error, which
   ended rouille's `run()` (a loop over `incoming_requests()`, whose `next` is `recv().ok()`),
   and dropped the listener. The control port was gone until a restart, while the flows' ports
   and their runs went on, and nothing said so (tiny_http speaks through `log`, which has no
   backend in Holon). An accept fails when the process has no file handle left (the data plane's
   sockets count against the same limit). A client that resets while it waits to be accepted
   doesn't fail it on Linux or Windows 11 (seen 2026-11-19: the connection is accepted, and its
   reset met at its first read), though some systems' documentation says it may. Now the thread
   waits and tries again, until the server closes: 5 ms, doubled while they fail in a row, to
   1 s (an accept that fails for want of a handle fails again at once), as Holon's data plane
   does. The first of a run is said on the server's error stream, with the system's words, and
   so is its end ("accepts connections again, after N that failed"); a run that starts within a
   minute of the last line is counted instead, and said with the next line, so a client that
   holds the server near its limit, runs ending and starting as handles come and go, gets a
   line or two a minute, not two a run. The error is no longer queued.
6. **A deadline for a whole request** (`src/limits.rs`, `Deadline`; `src/client.rs`;
   `src/util/refined_tcp_stream.rs`). The idle limit bounds each wait for a byte, not a request:
   a client that sent its request a byte at a time, each within the limit, held its connection
   for as long as it liked, and a body of 1 KiB or less, which tiny_http reads before the
   application sees the request, was out of reach of Holon's own deadline for a body. Now a
   request's line, its headers and such a body arrive whole within `limits::REQUEST` (30 s,
   Holon's deadline for a body; main.rs asserts it) of its first byte. Each read waits no longer
   than what is left of it (the socket's read timeout is the lesser of that and the idle limit,
   set again only when it changes); past it the client is told 408 and the connection closed, as
   for a read inside a head that waits out the idle limit. Either 408 now says why in words (as
   JSON, as the 503 past a bound does) with `Connection: close`, and is read before the close:
   what the client still sends is read off and thrown away until it closes, or for the linger
   (`Limits::linger`, 2 s, which a connection refused past a bound is kept for too: change 3; 64
   KiB at most), since closed with bytes unread the connection is reset, and a reset can reach the
   client before the 408 (on Windows it throws away a 408 not yet read). The deadline starts at the request
   line's first byte, so a kept-alive connection waits for its next request by the idle limit
   alone, and it ends once tiny_http has read the request whole, so a larger body, which the
   application reads, is held to the idle limit and the application's own deadline.

`Server::from_listener_limited` takes other limits; every other constructor (rouille's
included) uses `Limits::default()`.

## What else differs from the published crate

- The manifest: no SSL dependencies (the `ssl*` features keep their names, which rouille's
  features name, but turning one on fails to build: the control port is plain HTTP and TLS is a
  proxy's), no dev-dependencies (upstream's benches and examples need them), `publish = false`,
  doc examples not run, clippy's lints allowed in one place (`[lints.clippy]`, they are
  upstream's to take), and two rustc lints upstream's code trips today (`[lints.rust]`).
- Upstream's `benches/`, `examples/` and CI files are left out; `src/`, `tests/`, the licences
  (`LICENSE-MIT`, `LICENSE-APACHE`), `README.md` and `CHANGELOG.md` are as published.

## Not changed

- A write to a client that doesn't read waits as long as the client likes (upstream's
  `responding_to_non_consuming_client` shows it), holding its handler's thread and its place
  among the requests. A write limit beside the read limit would bound it, but a write cut part
  way leaves the connection taking requests behind a half-sent answer (and on Windows a timed-out
  send leaves the socket unusable), so it needs the connection closed on a failed write first.

## Keep in mind

- **`cargo audit` doesn't watch this copy.** It checks the crates `Cargo.lock` takes from a
  registry, and this one is a path. An advisory for tiny_http (RustSec, GitHub's) has to be
  checked by hand, and a fix carried over here.
- Taking a new upstream release means copying it over this folder and putting the changes
  above back (the diff above), with their tests.
