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
   never cut, and a response is never cut. The limit is past Holon's deadline for a body
   (`http::BODY_DEADLINE`), so a body that stops is told 408 by Holon first; main.rs asserts it.
3. **A bound on open connections** (`src/limits.rs`, `src/lib.rs`), in all (1,024) and from one
   address (512). Upstream accepted every socket and gave each a thread for as long as it was
   open. One past either bound is answered 503 at once and holds no thread. Its sending side is
   ended at once, and one thread for all refused connections reads off and throws away what
   their clients send until each client closes, or for 2 s (256 at most at once). A client that
   sent its request before reading, as every client does, then reads the 503, rather than a
   reset. One address may hold every request Holon answers at once (`MAX_IN_FLIGHT`, 512),
   which a proxy Holon trusts carries for all its clients; main.rs asserts both bounds hold
   Holon's in-flight bound. A Unix socket's connections count as one address.
4. **`Connection: close`** (`src/response.rs`, `src/request.rs`, `src/client.rs`). Upstream
   dropped a `Connection` header the application set. Now `Connection: close` is sent, and the
   response is the connection's last: no request after it is read and the client is sent the
   end of the stream. Holon's 408 for a body that stops relies on it.

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
- Taking a new upstream release means copying it over this folder and putting the four changes
  back (the diff above), with their tests.
