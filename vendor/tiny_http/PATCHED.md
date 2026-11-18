# tiny_http 0.12.0, patched for Holon

This folder is a copy of [tiny_http](https://github.com/tiny-http/tiny-http) **0.12.0** as
published on crates.io (git `212b1c45`), the HTTP server under rouille, which serves Holon's
control port (the API the editor and the commands use). The workspace's `Cargo.toml` points
every use of `tiny_http` at it (`[patch.crates-io]`), and it is a workspace member: `cargo test
--workspace` runs its tests (`src/util/task_pool.rs`, `src/limits_tests.rs`).

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
2. **An idle limit** (`src/limits.rs`, `src/client.rs`, `src/connection.rs`). Every socket read
   waits at most `limits::IDLE` (60 s). A connection that sends nothing of its next request by
   then, or nothing at all, is closed without an answer; one that stops part way through a
   request's head is told 408 (upstream's answer, which it gave only where a timeout was set). A
   request's bytes that keep coming, however slowly, and a response however long are not cut.
   The limit is past Holon's deadline for a body (`http::BODY_DEADLINE`), so a body that stops
   is told 408 by Holon first; main.rs asserts it.
3. **A bound on open connections** (`src/limits.rs`, `src/lib.rs`), in all (1,024) and from one
   address (256): one past either is answered 503 and closed at once. Upstream accepted every
   socket and gave each a thread for as long as it was open. main.rs asserts the bounds hold
   Holon's own on requests (`MAX_IN_FLIGHT`, `MAX_FROM_ONE`).
4. **`Connection: close`** (`src/response.rs`, `src/request.rs`, `src/client.rs`). Upstream
   dropped a `Connection` header the application set. Now `Connection: close` is sent, and the
   response is the connection's last: no request after it is read and the client is sent the
   end of the stream. Holon's 408 for a body that stops relies on it.

`Server::from_listener_limited` takes other limits; every other constructor (rouille's
included) uses `Limits::default()`.

## What else differs from the published crate

- The manifest: no SSL dependencies (the `ssl*` features keep their names, which rouille's
  features name, but turning one on fails to build: the control port is plain HTTP and TLS is a
  proxy's), no dev-dependencies, `publish = false`, doc examples not run, and clippy's lints
  allowed in one place (`[lints.clippy]`): they are upstream's to take.
- Upstream's `tests/`, `benches/`, `examples/` and CI files are left out; `src/`, the licences
  (`LICENSE-MIT`, `LICENSE-APACHE`), `README.md` and `CHANGELOG.md` are as published.

## Keep in mind

- **`cargo audit` doesn't watch this copy.** It checks the crates `Cargo.lock` takes from a
  registry, and this one is a path. An advisory for tiny_http (RustSec, GitHub's) has to be
  checked by hand, and a fix carried over here.
- Taking a new upstream release means copying it over this folder and putting the four changes
  back (the diff above), with their tests.
