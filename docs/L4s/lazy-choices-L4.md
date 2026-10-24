# Lazy choices: a branch not taken doesn't run (L4)

**Status:** built (2026-10-25). An evaluator change in core, with what it touches in the host
and the editor's replay. It changes no hash, no stored unit and no value a flow produces: only
what the host is asked to do, and when. One change from the first draft: `select` is not lazy
(§2), since its meaning needs both sides.

---

## 1. The bug

`eval_composite` evaluated every child, in one topological pass. The choice leaves (`guard`,
`coalesce`) only decided afterwards which value flows on. That's harmless for pure
nodes, but wrong for nodes that ask the host:

```
respond if req.json.go then http.get("httpbin.org", "/json") else "skipped"
```

lowers to `coalesce(guard(go, http_request(…)), "skipped")`. The request's path is a constant, so
`http_request` fires in the first round whatever `go` is, the host sends it, and the `guard`
throws the answer away. It compiles with no warning, the manifest lists the call, and the request
goes out on every run. The same holds for:

- an `await` in a branch not taken: the run parks, waiting for an event the branch it took never
  needed;
- a collection read: made and recorded, so a stateful run may be retried over a read it didn't
  use;
- a holon called in a branch: entered and run in full, along with anything it asks.

It is also why `match` (match-L4.md) couldn't allow requests in its arms.

## 2. The rule

**A choice evaluates only the inputs it needs, given the ones it has already seen:**

| Leaf | Evaluates | Then |
|---|---|---|
| `guard(cond, value)` | `cond` | `value` only if `cond` is `true` |
| `coalesce(a, b)` | `a` | `b` only if `a` is absent |

Every other leaf needs all its wired inputs, as before. That includes `select(c, a, b)`: it is
the strict choice (LANGUAGE.md §6.4), absent when either side is, whatever `c` says. Skipping the
side not chosen would turn that absent into a value, a change of meaning, so `select` needs, and
runs, both sides. A node is evaluated when something evaluated
needs it, or when nothing consumes it at all (§3). A node that only feeds choices that don't need
it is never evaluated: it is **not run**.

**Why this changes no value.** Every leaf is pure and total. `guard(false, v)` is absent and
`coalesce(a, b)` with `a` present is `a`, whatever `v` and `b` are. So for every node that is
evaluated, the lazy and eager evaluators compute the same value, and every output is the same.
What changes is the set of nodes evaluated. That is visible only through what those nodes ask
the host (the bug) and in the run's timeline (§6).

## 3. Which nodes run

The evaluation starts from the composite's **roots**:

- every child wired to a boundary output (a route's `respond` and its effects, which `route_spec`
  exposes as outputs);
- every child none of whose outputs is wired anywhere. Today a node nobody consumes is still
  evaluated, and an `http_request` placed on the canvas with its response unused still sends.
  Keeping such nodes as roots keeps that behaviour. The language already refuses an unused value,
  so this only matters for canvas-built flows.

From the roots, evaluation pulls what each node needs: all wired inputs for an ordinary leaf, or
§2's rule for a choice. It runs depth-first with an explicit stack (no recursion depth to
overflow), roots in ascending child index and inputs in port order, so it is deterministic. A
node needed by two consumers is evaluated once.

**A holon's inputs are evaluated before it is entered**, as a call's arguments are. Laziness works
at the level where the choice sits: `if c then fetch(x)` doesn't enter `fetch` when `c` is false.
But a holon `h(c, v) = v when c`, called as `h(go, http.get(…))`, gets its request evaluated by
the caller, which can't see the choice inside `h`. Reaching through a boundary would mean passing
unevaluated inputs into composites, a much larger change. The rule is stated in LANGUAGE.md §6.4,
and the leak test (§8) covers the case that works.

`map` and `fold` are unchanged: they apply their function to every item (`map` applies every one
before giving up, so a round's requests go out together).

## 4. Pending is not absent

Within a round, an `http_request` whose answer hasn't come yet produces nothing, and today that
looks exactly like absent. With laziness the difference matters:

```
price = cache[k] ?? http.get("prices.example.com", "/{k}").json.price
```

Suppose the cache read itself depends on an earlier request's answer. Then in the first round
`a` is "not yet". If `coalesce` treated it as absent, it would send the fallback request before
learning it isn't needed. So the evaluator carries a third state:

- **Pending:** an asking node the host hasn't answered yet (a request not yet performed, an
  `await` whose event hasn't come), or any node that needs a pending input.
- **Absent:** nothing will come, which is what it means now.

The choices wait on pending:

| Leaf | Its deciding input is pending | Then |
|---|---|---|
| `guard` | `cond` | pending; `value` isn't evaluated |
| `coalesce` | `a` | pending; `b` isn't evaluated |

**Where the third state lives.**

- `Observer::answer` returns `Answer::Value(v)`, `Answer::Absent` or `Answer::NotYet` instead of
  `Option<Value>`.
- `Asking` (rounds.rs) says `NotYet` for a request it hasn't performed (it also records it as
  pending, as now) and for an `await` whose event hasn't come. Collection reads are definitive
  (`Value` or `Absent`), as are `Recorded` (a replay: a request the record lacks is absent) and
  `NoObserver`.
- Inside a composite, each child's slot is `Value`, `Absent` or `Pending`. Pending crosses
  composite boundaries and `map`/`fold` applications as pending, so a holon or a `map` waiting on
  an answer is waiting, not failed.
- At the public edge (`eval`, `eval_observed`) pending becomes `None`, as it is today.

**The round loop is unchanged.** It already runs another round while requests are pending, and
parks while an `await` waits. A choice that depends on an answer may now take one more round,
because the branch it takes is only known once the answer comes, and only then are the
branch's requests asked. That is the correct cost: the requests that go out are the ones the run
uses.

**The invariant:** in a run's final round nothing is pending, unless the run waits (and then
nothing is performed). That holds already, since `NotYet` comes only from requests (which make
another round) and `await` (which parks), and the final round's outputs and effects are exactly
the eager evaluator's for the same answers.

## 5. What it costs

- One more round for each choice whose deciding input waits on an answer, before the branch's
  own requests. Run budgets count requests and time, not rounds, so they are unchanged.
- Evaluation order changes from one topological pass to demand order. Values don't depend on
  order (pure leaves). The timeline does (§6), and requests are already sorted by path when sent.
- A branch not taken costs nothing at all: often less work than today, never more.

## 6. Seeing it: the run log and the editor's replay

**Server.** A node not run has no `enter`/`leave`, so the run's timeline doesn't list it. That
absence is the record: the flow log needs no new field, and `holon verify` re-runs with the same
answers, takes the same branches and asks the same requests. (Building this found that verify
answered a run's requests only from the earlier parts of a durable run, never from the run's own
rounds, so any run that made a request failed to check. It answers from both now.)

**Editor replay.** `replay.ts` walks the canvas eagerly (Kahn order, every node evaluated), so it
would compute the untaken `http_request` and show it without a response. It changes in three
ways:

1. **It follows the same rule.** The walk pulls from the run group's roots, and a canvas
   `guard` or `coalesce` node pulls only what §2 says. The rule lives in core and is
   exported through wasm (one function: given a choice leaf and the values of the inputs seen so
   far, which input to evaluate next), so the editor doesn't keep its own copy.
2. **A node not run is shown as not run.** It is dimmed on the canvas, its inputs and outputs are
   left blank instead of shown as absent, and the Inspector says
   "not run: the guard after it didn't take this branch" (or the coalesce after it already had a
   value). It names the choice that skipped it, which is the first choice on its way to the
   roots.
3. **The diagnosis leaves it out.** "Why a run failed" finds origins and starved nodes among the
   nodes that ran. A branch not taken is neither.

Where the run's timeline reaches (canvas nodes, and one level into holons), replay checks itself
against it. A node the run evaluated that replay didn't, or the reverse, means the replay differs
from the run, and view mode says so, as it does for a differing answer. A run logged before this
change has every node in its timeline, and replay shows its untaken branches as not run anyway,
since that is what the rule says. For those older runs the timeline cross-check is skipped.

**Design mode** (the editor evaluating a flow while it's edited) goes through core's `eval`, so
it is lazy without changes. It shows nodes not run the same way as replay.

## 7. What it does not change

- **`and` and `or` stay strict.** `true || absent` is absent today, and short-circuiting would
  make it `true`. That's a change of value, not of what's asked, so it doesn't belong in this
  change. A request in the second operand of `||` is still sent. If that matters, it gets its
  own design.
- **`default`** has no subgraph to skip: its fallback is a literal param.
- **Hashes, the preimage and `ENCODING_VERSION`:** unchanged. Laziness is the evaluator's, not
  the unit's.
- **The firing rule:** a node with a wired input absent still doesn't fire. Laziness only
  decides which nodes are asked to fire at all.

## 8. Tests

- **Core, differential.** The current eager evaluator stays as a test-only oracle. For every
  sample's units and for generated graphs of pure leaves, lazy and eager outputs are identical.
  This is the property of §2, checked rather than argued.
- **Core, the rule:** with an observer that counts what it is asked,
  - a request under a `guard` whose `cond` is false is never asked;
  - a request under `coalesce`'s `b` is never asked when `a` is present;
  - `select`, strict, asks both sides;
  - a holon under an untaken guard is never entered;
  - a canvas-style node nobody consumes is still asked (§3).
- **Core, pending:**
  - `coalesce(pending a, request b)`: `b` isn't asked in round 1. When `a` arrives present, `b`
    is never asked; when `a` arrives absent, `b` is asked in round 2.
  - A `guard` on a pending `cond` waits the same way.
  - Pending crosses a holon boundary and a `map` as pending.
- **Host:**
  - the leak (§1), before and after: the `if` with `go: false` sends nothing, and with
    `go: true` sends once;
  - an `await` in an untaken branch doesn't park the run;
  - an untaken read isn't recorded;
  - `holon verify` passes on runs that took different branches.
- **Editor:**
  - `replay.test.ts`: untaken nodes come out as not run, with the choice that skipped them; the
    diagnosis ignores them; the timeline cross-check flags a mismatch.
  - e2e: the sample's two runs replay exactly, and view mode shows the fallback request as not
    run in the run that hit the cache.
- **Sample flow `cache-aside`:** an id per name, from a collection when it has one, else fetched
  (a fresh uuid from httpbin) and stored.
  ```
  collection ids

  route GET /id/:name (req) {
    name = req.params.name
    id = ids[name] ?? parse_json(http.get("httpbin.org", "/uuid").body).uuid
    ids[name] = id
    respond id
  }

  route PUT /id/:name (req) {
    ids[req.params.name] = req.json.id
    respond req.json.id
  }
  ```
  Its two runs for one name show the change: the first fetches, and the second answers the same
  id and makes no request at all, which its log's rounds and the replay's "not run" both show.
  The `PUT` route sets an id, so the e2e test needs no network.

## 9. match-L4

`match` lowers to `guard` and `coalesce`, so with this change an arm not chosen doesn't run, and
its requests and `await`s aren't asked. match-L4 §7's refusal of requests in arms is dropped,
and so is the separate `if` check it proposed: the leak is fixed, not forbidden. This lands
first; `match` builds on it.
