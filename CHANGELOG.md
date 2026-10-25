# Changelog

## 0.1.0-rc.20

Nothing to redeploy: no unit is hashed differently, and flows deployed with rc.19 keep running.
A log written by rc.20 has fields an older reader skips; a policy may now set `claim_wait_ms`.

- **Key claims** (docs/L4s/key-claims-L4.md). Two requests with one idempotency key, sent in
  the same instant, both called the processor: the optimistic commit and its retry kept one call
  per *run*, never one per *key*, because an outbound call leaves mid-run, before the host knows
  whether the attempt will be kept. Now a run about to send its first outbound request claims
  every collection key it read that the flow may write, re-checks those reads against the live
  state, and holds the keys until it commits, is discarded or parks. One that finds a read changed
  as it claims it runs again from the new state having sent nothing; one holding keys and needing
  another run's lets go, waits and runs again, so nothing deadlocks; a commit waits for foreign
  claims on what it writes. Nothing is declared and nothing is in the manifest: the claim is
  derived from the run's reads and the grant, and a run that sends nothing claims nothing, so a
  read-modify-write with no call (the `likes` sample) stays as it was. State cells are never
  claimed: a cell is one per deployment, and claiming it would serialize every calling run.
  `claim_wait_ms` (10 s by default, ceiling 60 s) bounds the wait; past it the run answers 503 as
  busy, naming the key. Verified live: the race that sent two now sends one, the second run
  waiting 907 ms and replaying the first's record.
- **Claims shown.** `runs` says "retried ×1" and "waited 681 ms for a key" on a line that has
  them. `explain` says under the read it protects what the run claimed, how long it waited and
  held the key, or that it found what it read had changed and ran again having sent nothing.
  `holon check` and `show` say, apart from what a flow may do, *while it runs: holds the keys it
  reads in "charges" from its first outbound call until it commits*, for a flow that calls out
  over a collection it both reads and writes.
- **A new sample, `idempotent`**: one charge per Idempotency-Key. The same key with the same body
  answers the first charge again, the same key with a different body is refused (409), a key
  expires after a day, and the processor is called only when there is no prior charge, since
  `prior ?? fresh` never asks for the call on a replay.
- **A new sample, `tally`**: post a CSV and get each column's kind, the min, max and mean of a
  number column, the distinct values and top three of a text column; parsing text into structure
  is a js node's work, and the flow around it keeps the last summary in its state.
- The js engine's cost measurement (`what_a_call_costs`, on request) prints wall time too, cold
  and warm, and says which build it ran on.

## 0.1.0-rc.19

Flows deployed with rc.18 keep running. A flow with a `js` node redeploys as a new version
(the engine module was rebuilt for `check`), with nothing new to approve; a flow saved by rc.18
opens as before and saves smaller.

- **A js node that gives nothing says why.** `explain` names the engine's reason under the
  statement that placed the node: a body's exception as the engine words it ("`deep` gave
  nothing: TypeError: cannot convert 'null' or 'undefined' to object (the body's line 2, col
  19)"), or the engine's own failure ("out of fuel (200000000 instructions)"), for a node bound
  by name or inlined into `respond { … }`. The run records it beside the engine's step.
- **`holon check` compiles each js body** once, running nothing, and says a syntax error at its
  line in the file with the line and a caret, like any other mistake. A body that compiles passes;
  what it does at run time is the run's to say.
- **A flow document carries a module by engine and params, not by its binary.** The js engine's
  preimage is 3.2 MB (6.5 MB as hex) and each budget made another copy in every saved flow,
  deploy body and deployment entry; a flow with two budgets could not deploy at all (the control
  plane's 8 MiB body limit). A document now lists `{hash, engine, params}` under `modules`,
  rebuilt on open from the engine the build carries and checked by hash; a deployment entry for
  the `nearest` sample is 21 KB instead of 6.5 MB, and two budgets deploy.
- **Measured** (docs/L4s/js-node-L4.md F6): on a release build a trivial js body costs 6 ms, the
  `nearest` search 20 ms, a request through the deployed sample 50 to 60 ms more than one
  without a js node; a debug build is eighty times slower.

## 0.1.0-rc.18

Nothing to redeploy: no unit is hashed differently, and flows deployed with rc.17 keep running.

- **JavaScript as one node** (docs/L4s/js-node-L4.md, `docs/LANGUAGE.md` §5.8a). `js name =
  (inputs) -> outputs { body }` is a top-level definition beside `holon`, for the one
  computation the vocabulary can't spell. It runs on a JavaScript engine (Boa) that is itself a
  module: a fresh realm per call, no clock, no randomness, no timers and no network, so the same
  call gives the same answer. Budgets default to 200M instructions and 16 MiB per call (`fuel N`,
  `memory N` change them). `fmt` writes the statement back with its body as typed. Not yet: a
  body that throws, doesn't parse or runs out of fuel makes the node absent, but `explain`
  doesn't show the engine's message, and `holon check` doesn't parse the body. The engine
  rides in the editor's bundle, which grows from 1.9 MB to 6 MB.
- **Modules run** (docs/L4s/wasm-module-L4.md). Everywhere a flow runs (a request, a clock run, a
  resumed durable run, a deploy's first run, `holon verify`'s rerun, and design mode in the
  editor), a module runs under its budget. A deploy's manifest carries the budgets as
  `Limit(module_fuel=…)` and `Limit(module_memory=…)`, `holon check` says "run code it carries"
  with them, and the policy holds them to ceilings (`[ceilings]` raises them).
- **A new sample, `nearest`**: a dictionary that answers the closest word to a query by edit
  distance, the one computation a `js` node does while the flow around it stores, routes and
  answers.
- **The text and the canvas, linked by expression.** The cursor in a flow's text picks the node
  under it (`base64` in `base64(uppercase(req.body))`, not the whole line), and a node picked on
  the canvas marks the expression it is written as. It works in view mode too, where the text
  also shows the run: what didn't run is dimmed (the values of a `match`'s arms not chosen), the
  step is marked as you step, and the values of what you named are written beside it. On a
  window 1600px wide or more, the text pane is open by default.

## 0.1.0-rc.17

**Run `holon migrate` where you used to start `holon serve`.** A server keeps its state in
`~/.holon/data` now, wherever it is started from, so one started as before begins empty; the
old `.holon-*` files are left where they are, and `holon serve` says so when it sees them.

- **Everything in one `~/.holon`** (or `HOLON_HOME`): `data/`, `keys/`, `run/`, `tokens.json`,
  `mcp-hosts.toml` and `bin/`. `holon serve` needs no `--data`; it prints the folder it uses.
  `--data`, `HOLON_DATA` and each per-file flag still win.
- **`holon migrate`** moves what an older release left elsewhere: a server's `.holon-*` files
  (from the current folder, or `--from DIR`) into `~/.holon/data`, and key files from
  `~/.config/holon/keys` into `~/.holon/keys`. It says what it will do and asks (`--yes` doesn't
  ask, `--check` exits 1 when there is something to do), refuses while a server is running,
  checks that the secrets open with their key before anything moves, and copies and checks
  before it removes.
- **A secrets file records which key it uses** (format 3, `key_id`), so a data folder can be
  moved or restored from a backup and its secrets still open. A format 2 file is upgraded on
  open, keeping its key where it is.
- **`uninstall --purge` removes all of `~/.holon`**: data, key, run files, tokens and the list
  of agent hosts set up.
- **A module is a unit** (docs/L4s/wasm-module-L4.md), underneath only: a WebAssembly body with
  no imports, admitted by its exports and budgets. No host runs one yet, so nothing a flow does
  changes, and no unit already encoded is hashed differently.

## 0.1.0-rc.16

Nothing to redeploy: no unit is hashed differently, and flows deployed with rc.15 keep running.
What changes for them is what runs (below): a request in a branch not taken is no longer sent.

- **A branch not taken doesn't run.** `guard` and `coalesce` evaluate only the inputs they need:
  `if c then a else b` runs `a` only when `c` is true and `b` only when it isn't (or `a` has no
  value), `a when c` runs `a` only when `c` is true, and `x ?? y` runs `y` only when `x` has no
  value. So a request in an untaken branch isn't sent, an `await` there doesn't park the run, a
  read there isn't made, and a holon there isn't entered. No value changes. A choice that waits on
  an answer waits a round instead of deciding early. `select` stays strict and runs both sides,
  and a holon's inputs are still computed before it is entered. View mode dims the nodes a run
  didn't run, and the Inspector names the choice that skipped them. New sample: `cache-aside`.
- **`match`: a choice by value.** `match op { "add" -> a + b, "div" -> a / b, else -> 0 }`
  chooses the arm whose pattern equals the subject. Only that arm runs, and if it has no value
  the match has none (so `respond … missing: 400` answers 400), where a chain of `if` would fall
  through to its `else`. Patterns are literals; an arm may take several (`"sub", "minus" ->`).
  It lowers to `equals`, `guard` and `coalesce`, and `holon fmt` prints those back as `match`.
  `match` is now a keyword. New sample: `calc`.
- **`range(n)`** is the list `[0, …, n-1]`, for a whole `n` up to 10000 (nothing otherwise), so
  a `map` can count and still ends. New sample: `tickets`.
- **`holon verify` checks runs that made requests.** It answered a run's requests only from the
  earlier parts of a durable run, so any run that sent a request failed to check.

## 0.1.0-rc.15

**Delete the flows you deployed before this release, then deploy them again.** Some units are
hashed differently from this release on (below), and a server that finds a deployment stored under
the old hash refuses to start:

```
holon: deployment "…" in "…deployments.json": store error: Corrupt { … }
holon: refusing to start rather than overwrite it — fix the file or move it aside
```

To delete them, stop the server, move `deployments.json`, `deployments.logs` and
`deployments.activity.jsonl` out of its data directory (`~/.holon/data` unless you gave `--data`),
and start it again. Accounts and saved flows stay; open each saved flow and deploy it again. Their
runs' logs go with the old deployments.

- **Same-hash children are told apart by where their values go.** Two identical nodes in one
  route used to be ordered by a search with a cap, which could fail on a large route. Each child
  is now ordered by a hash of the path its values take to the route's outputs; a composite without
  such twins keeps its hash. Two identical nodes whose values go nowhere are an error: "the same
  value is computed twice and never used, so nothing tells the two apart: remove one, or use it".
  This is the change that re-hashes units.
- **A lambda uses what is around it.** `map(keys(r), fn: fn(k) -> r[k])` now compiles: a name a
  function uses from the route or holon it is written in becomes an input of the function, and
  `map` and `fold` pass it after the item. The explicit form, `map(xs, r, fn: fn(k, r) -> r[k])`,
  is the same unit. A function of the wrong arity is said at the function. `holon fmt` prints
  one-expression functions back as lambdas. New sample: `quote`.
- **A read inside a mapped function is answered.** A collection read, an outbound request or a
  waiting point inside a `map` or `fold` function came back absent, silently. Each item's is now
  answered, recorded and replayed on its own, and a round sends every item's requests together.
  View mode replays such a `map` from what the run recorded. New sample: `basket`.
- **`holon mcp` reads back what is there:** `deployments`, `flows` and `save` tools, and a run
  records who sent it.
- A wiring-form file whose unit hash starts with a digit reads again.
