# Changelog

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
