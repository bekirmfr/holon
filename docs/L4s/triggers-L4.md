# Triggers: a flow with several (L4)

**Status:** design, for review, before any code (2026-10-26, revised 2026-10-27). Its claims are
checked against the code at `d4223a8` (rc.27, with databases, plus an editor fix) and cited by
file and line there. Reviewed once: what the review decided is in §11. What the second and third
checks of the code corrected is in §12 and §13.

**The words.**

| word | means |
|---|---|
| **trigger** | what starts a run |
| **handler** | a trigger and the block it runs |
| **route** | a handler a request on the flow's port starts: open to the network |
| **clock** | a handler time starts: `every 2s { … }` |
| **file** | a handler a file settling in (or leaving) a folder starts: `on file inbox (f) { … }` |
| **action** | a handler a person, an agent or another flow starts through Holon, as its grant allows; never the network: `action reset (input) { … }` |

"Trigger" is the word the product already uses for what starts a run: the manifest's `trigger`,
the permissions manual, the deployed page's "triggers", `/trigger`. "Driver" is not used for it:
in `holon-core/src/kind.rs` (182) a `Kind::Driver` capability is any value from outside a run
(file reads, secret reads, randomness too).

Today a flow has exactly one trigger: it is a server (routes), a clock flow (`every`), a flow that
runs on files (`on file`), or one run by hand (`ingress`, run through `/trigger`, which also fires
a clock or file flow's whole unit). The trigger should not decide what a
flow can be. A flow is a set of **handlers**, and the handlers share everything the flow owns:
its state, collections, channels, folders, databases, secrets and grant.

```
flow ticker
port 8129
channel ticks

every 2s {
  n = (state.n ?? 0) + 1
  state.n = n
  ticks.send({ n: n, at: now })
}

route GET / (req) {
  respond page(), headers: { "content-type": "text/html; charset=utf-8" }
}

route GET /live (req) {
  respond ticks.subscribe(from: "kept")
}

action reset (input) {
  state.n = 0
}
```

A clock that streams to whoever is watching, with nothing outside the host, and a reset an
operator runs from the editor: the sample this L4 ends with. Today it can't be written: `every`
with routes is ignored, a channel reaches only subscribers on its own flow's routes
(`holon/src/channels.rs` 1–2), so a clock flow can't stream to another flow's page, a route can't
hold a statement that doesn't use its request, and a server can't be run by hand (§1).

---

## 1. What is already there

All paths are under `crates/` unless they start with `docs/` or `editor/`.

**The language.**

- **Headers and items interleave freely** (`holon-author/src/lang/parse.rs`, `file` from 268).
  `every` takes a duration or seconds (`header`, from 294). `on file <folder> [deleted] [matching
  "<glob>"]` is a header too (ending 333). `route M /p (name) [resumes x] { … }` is an item (286,
  `route` from 481). The `database` header (rc.27) joins the others and is allowed in either kind
  of flow.
- **Two rules keep a flow one kind** (`lang/lower.rs`, `Lowerer::run` from 295):
  - `on file` with routes is an error: "a flow with routes runs on requests: `on file` is for a
    flow without routes, whose runs start with a new file" (322).
  - With routes, the top level holds only literal bindings, each placed afresh in the routes that
    use it, and the wiring form's `node` and wire lines (325–339). Another binding is refused with
    "routes are separate units; put shared logic in a holon (only a literal may be bound at the top
    level)" (335), and any other statement with "with routes, a statement goes inside a route"
    (337). `docs/LANGUAGE.md` §5.11 (1135): "Two routes may
    not share a node".
- **Several `on file` lines already work.** `run` refuses a bad glob and the same line written
  twice, and a flow that declares `on file` but never uses `ingress`. `ingress` is then the
  `coalesce` of the lines' `file_arrival` leaves, in the order written (`ingress`, lower.rs from
  1274; LANGUAGE.md §5.1, 745). The `file-audit` sample has three.
- **`every` with routes is silently ignored.** `settings()` (lower.rs from 386) turns `every` into
  the deploy setting `interval` whatever the flow is (392). The host then drops it for a server
  (`holon/src/deploy.rs` 149–151, 212–222). Checked with `holon check` at `1ccf376`: a flow with
  `every 2s` and a route checks clean, as "a server on :8129" that may only "receive requests".

**A route is everything wired to its request.**

- `find_routes` takes each connected group of the canvas holding one `request` node
  (`holon-author/src/routes.rs` from 63); groups without one are skipped silently.
- The lowering checks that every node of a route's block lies in its request's group (lower.rs
  372–382). So a route can't count its own hits:

  ```
  route POST /hit (req) {
    state.n = (state.n ?? 0) + 1      # error: this isn't wired to the route's request
    respond 204
  }
  ```

  Checked with `holon check` at `1ccf376`: "this isn't wired to the route's request, so it isn't
  part of the route (a route is everything wired to its request): use the request in it, or move it
  to a flow of its own" (lower.rs 380; a unit test pins it at 2238).

**`respond` already carries its trigger; no other sink does.**

- `respond` takes the request on an input it never reads: "that wire is what makes a route one
  connected group even when the handler never looks at the request" (`holon-author/src/templates.rs`
  10–13; the lowering wires it, lower.rs 769). That is why `respond 204` is part of its route.
- The other sinks have no such input. These are what this L4 calls sinks: the writes and the
  calls out.
  - The writes are the leaves whose output is an `Effect` (`holon-core/src/catalog.rs` 267–354):
    `store_write`, `collection_put`, `collection_delete`, `file_write`, `file_delete`, `file_copy`,
    `file_create`, `file_create_copy`, `db_exec`, `db_insert`, `db_update`, `db_delete`,
    `channel_send` and `egress`.
  - The calls out are `http_request`, `http_upload` and `http_stream`, which return a `Record`.
  - All of them are part of a route only through their values.
  - A fifteenth Effect leaf, `channel_subscribe` (351), is a read: a route's streamed answer.
  - The state, collection and database writes are `Kind::State` in `kind.rs` (183), not
    `Kind::Sink`. `Spawn` is a Sink capability that no leaf has.
- So the way to count hits today is to tie the write to the request by hand. One way is to
  respond with the value written (`n = (state.counter ?? 0) + 1; state.counter = n; respond n`).
  Another, when the route answers nothing, is to test the request:
  `state.n = (state.n ?? 0) + (1 when is_truthy(req))` with `respond 204` checks clean, because
  a request record is never falsy (primitives.rs 194). The same trick drops the write when the
  value it tests is falsy (`guard`, `primitives.rs` 200–204: no value, no effect).
- **A holon runs as one.** When it runs, every node inside runs, including a write that reads none
  of its inputs (`eval_composite`, `holon-core/src/eval.rs` 359–370). `holon check` passes this:

  ```
  holon h(x) -> (y, saved) {
    y = x + 1
    saved = state.set(n, 0)
  }

  route POST /h (req) {
    r = h(req.json.by)
    respond r.y
  }
  ```

  Its placement is what must be tied. With `holon bump() -> saved { saved = state.set(n, 0) }`
  or `holon bump(x) -> saved { saved = state.set(n, x) }`, both `bump()` and `bump(1)` in a route
  fail with the error above.

**The units.**

- **A server** is a composite with `declare=server` and `port`, no edges and no ports, whose
  children are endpoints (`routes.rs` `server_unit` from 216). `holon-core/src/declare.rs`
  `read_server` (from 75) reads it, refuses wiring between the children, and refuses two routes of
  the same method and shape (110–113).
- **An endpoint** wraps one route's handler: `declare=endpoint`, `method`, `path` and optionally
  `resume`, one `request` input, the response then the effects as outputs (`routes.rs`
  `endpoint_unit` from 170; `declare.rs` `read_endpoint` from 134).
- **A flow without routes** is the whole canvas grouped as one composite (`holon-author/src/flow.rs`
  `flow_unit` from 149). A flow with routes deploys only its server: a group without a `request`
  is left out of what deploys.

**Capabilities and the manifest.**

- `Clock`, `NetworkIngress` and `FileArrival(folder)` are the trigger capabilities
  (`holon-core/src/kind.rs` 104–113, 182). A `declare=server` unit adds `NetworkIngress`
  (`unit_caps`, 669).
- `deployed(scheduled)` adds `Clock` to a flow deployed on a schedule: "The one place that rule is
  kept" (kind.rs 236–238).
- The manifest names **one** trigger: `NetworkIngress`, else the first `FileArrival`, else `Clock`
  (`holon-core/src/manifest.rs` `trigger`, 45). `permission_kind` (61) keeps that one trigger from
  being optional. The permissions manual says the same: "A flow's trigger is one of them"
  (`docs/manual/reference/permissions.md` 31).

**The host.**

- **Deploy** (`deploy.rs` `handle_post_deploy`, from 93) decides the kind:
  - the capabilities get `Clock` only for a non-server sent an interval (149);
  - `is_clock = caps.contains(Clock) && server.is_none()` (151);
  - file arrivals are watched from the unit's `file_arrival` leaves (152);
  - a unit with none of the three is refused: "this unit declares no driver capability" (153–158);
  - `interval_seconds` is required for a clock flow and dropped otherwise (212–222).
- **The clock** (`scheduler.rs`) fires each due deployment's whole unit with no inputs
  (`fire_one_clock_tick` from 64, `fire_with_state(root_unit, &[], …)` at 100), logs a `"tick"`,
  and sets `next_due` to the pass's time plus the interval (126). A pass's ticks run on scoped
  threads joined before the next pass (48), so a deployment's ticks never overlap: one that runs
  past its period makes the next due at once.
- **Arrivals** (`arrivals.rs`) look only at deployments without a server (`look_at` from 294, the
  filter at 299). `fire_one_arrival` (from 325) sources `FileArrival(folder)` with the handle,
  fires the whole unit, and logs a `"file"` run (381).
- **The data plane** (`listeners.rs` `serve_flow` from 76; `data.rs` `handle` from 92) matches the
  request to an endpoint and fires that endpoint with the request as its input (listeners.rs 138).
  It logs a `"request"` with `route = "M /path"` (143).
- **`/trigger/<slug>`** (`deploy.rs` 656–732) fires a non-server's whole unit with the body as
  `NetworkIngress`, logged as `"trigger"` (709). A server gets 400 (670).
- **The run log's `kind`** is `"request"`, `"trigger"`, `"tick"`, `"file"` or `"resume"`
  (`durable.rs` 636). `LogEntry.route` is "None for ticks, triggers, unrouted requests"
  (`recorder.rs` 69). The comment above `kind` (66) still lists only the first three.
- **What a log keeps of a secret** (rc.26) is the same for every run kind: tokens and secrets are
  redacted in `log_run`, and outputs or effects computed from a secret are withheld (`firing.rs`
  `logged_outputs` / `logged_performed`; a tick's history too, `scheduler.rs` 114–121).
- **`holon verify`** (`verify.rs` `rerun`, from 376) branches on `entry.route`, not on the kind
  (391):
  - with a route, it finds the endpoint in the deployed server and runs it with the logged
    request;
  - without one, it runs the whole deployed unit with no inputs.
  - Every rerun gets the clock run's limits with a one-hour budget (468), and only a request's
    status is compared (496–501).
- **The two run budgets** (`firing.rs` 130, `permissions.rs` 234–235): a run fired with
  `clock = true` gets `clock_run_budget_ms`, others `run_budget_ms`. Ticks (`scheduler.rs` 100) and
  file runs (`arrivals.rs` 380) pass `true`. Requests, triggers (`deploy.rs` 706) and resumed runs
  (`durable.rs` 634) pass `false`.
- **Concurrent runs** commit only if what they read is unchanged, or run again
  (`OPTIMISTIC_ATTEMPTS = 4`, firing.rs 101; `still_as_read` from 415). Key claims order the ones
  that call out (`claims.rs`, waited on at firing.rs 178; `docs/L4s/key-claims-L4.md`). Commit
  locks and run locks (rc.27) keep runs of several deployments that touch one file or database
  from writing over each other. Resumed runs now run again too, from where they waited.

**Runs by hand.**

- A flow with no route, clock or file header that reads `ingress` is run by hand:
  `POST /trigger/<slug>` with a body (`deploy.rs` 656–732). Its capability is `NetworkIngress`,
  the one a server's open port has, so its approval reads "receive requests"
  (`holon-author/src/describe.rs` 43) though nothing listens. The deployment lists its trigger as `Ingress` (`deploy.rs` `trigger_labels`, 29–40).
- `/trigger` refuses only a server (`deploy.rs` 669–670). A clock or file flow can be fired
  through it too, its whole unit run once; only the deployed page's button is limited to
  `Ingress` deployments (`DeploymentCard.tsx` 98).
- The deployed page runs it: a "request body" box and a **trigger** button on a running `Ingress`
  deployment, the outputs shown under it (`DeploymentCard.tsx` 98, 212–223).
- Editors and admins may call `/trigger`; viewers only read, and an agent may not
  (`access.rs` 125–132; `agent_may`, 78–85).
- A request sent through the control plane (`POST /deployments/<slug>/request`, `http.rs` 281;
  the editor's **send live**, `holon mcp`'s `call`) is logged with who sent it, `LogEntry.by`
  (`recorder.rs` 128–132, set at `listeners.rs` 197). A `/trigger` run names nobody.

**Durable runs.**

- A run of any kind can wait: `park_if_waiting` (`durable.rs` from 518) is called for a request
  (listeners.rs 140), a trigger (deploy.rs 710), a tick (scheduler.rs 102) and a file (arrivals.rs
  382).
- Only a request on a route with `resume` can deliver an event (`listeners.rs` 108–124); the token
  is claimed first, so it resumes its run once (`claim_by_token`, durable.rs 588). So a waiting
  clock or file run can only expire: `expire_waiting` (durable.rs from 685) delivers
  `{expired: true}`.
- A parked run keeps only `Clock`, `Random` and `NetworkIngress` among its sourced values
  (durable.rs 386), so a waiting file run loses its `FileArrival` handle. `capability_named`
  (durable.rs 320) already reads `FileArrival(…)` back, so widening that filter alone would keep
  it. A parked run is sealed under the secrets key on disk (rc.26), whatever it keeps.

**Persistence.**

- `Deployment` keeps `interval_seconds` (`holon/src/main.rs` 364), and `next_due` and `history`
  as "`Clock`-only" (359). The server is re-derived at load, never persisted (399).
- `DeploymentRecord` persists `interval_seconds` but not `next_due` (`wire.rs` 220–264). On load a
  clock flow is due at once (`persist.rs` 156).
- **A server never gets the clock or files, in six tests of `server.is_none()`:** deploy.rs 149 and
  151, persist.rs 148, 153 and 156, and arrivals.rs 299. (A seventh, listeners.rs 307, refuses a
  control-plane request to a flow that isn't a server.)

**The editor.**

- It deploys the server hash when the canvas has routes, otherwise the whole canvas
  (`useDeployFlow.ts` 95–104), and sends an interval only for a non-server (110).
- The ⚙ panel's "interval (s)" is "how often a flow without routes runs on the clock"
  (`DeployBar.tsx` 255).
- A replay picks the trace by `kind === "request"` (`replay.ts` 225–226, 247–250).
- The "server ▾" panel (`ServerPanel.tsx` 148) lists the routes, each with a "test" button
  (160–166).
- The flows page shows no port or interval. The deployed page shows a deployment's triggers and
  port (`DeploymentCard.tsx` 109, 168).
- **There are no frames on the canvas.** "Group" freezes a selection into a composite node
  (`canvasOps.ts` `groupSelection`), and a composite opens in an overlay of its own
  (`CompositeOverlay.tsx`). A route's nodes are known (`RouteGroup.nodeIds`, `derive.ts` 9) but
  nothing draws them.
- **A node has no handler or route field.** Its data is `PrimitiveData` (`PrimitiveNode.tsx`) and
  `NodeData` in Rust (`holon-author/src/canvas.rs` from 33), whose flattened `extra` (52) carries
  unknown fields through.

---

## 2. Handlers

A flow is its headers, its definitions (`holon`), and its **handlers**. There are four kinds:

| handler | its trigger | its runs | capability |
|---|---|---|---|
| `route M /path (req) { … }` | a request on the flow's port, matching | one per request, `req` the request | `NetworkIngress` |
| `every 2s { … }` | the clock | one per period | `Clock` |
| `on file inbox [deleted] [matching "…"] (f) { … }` | a file settling in (or leaving) a named folder | one per file, `f` its handle | `FileArrival("inbox")` |
| `action reset (input) { … }` | a person, an agent or a flow its grant allows, through Holon | one per call, `input` what was sent | `Action("reset")` |

- **One run is one handler's unit.** A run is a request's endpoint today. A tick runs its `every`
  block's unit, a file its `on file` block's unit, and a call its `action` block's unit. Nothing
  else in the flow runs.
- **Everything the flow declares is shared:** `state`, each `collection`, `channel`, `folder`,
  `database` and secret, and the grant. A route can read what a tick wrote, a tick's `ticks.send`
  reaches a route's subscribers, and an action can reset what both use. That is the point: one
  deployment, one log, one hash chain, one approval.
- **Runs of different handlers are concurrent runs.** They already are between routes (§1,
  concurrent runs): a run commits only if what it read is unchanged, or runs again, key claims
  order the ones that call out, and commit and run locks hold across deployments. A tick and a
  request that both bump `state.n` are like two requests bumping one key today (the `likes`
  sample's `POST /posts/:id/like`, `likes[id] = (likes[id] ?? 0) + 1`), nothing new. No sample has two kinds of run on one state yet: `ticker`
  is the first.
- **A handler's runs keep their own limits,** by the `clock` flag as today (§1): a request run
  and an action run get the request budget (as `/trigger` runs do), a tick or file run the clock
  budget.
- **Ticks of one handler never overlap.** The next is due one period after this one started, and
  not before it ends, as today. Two `every` handlers of one flow may tick at the same time, as
  two requests may.
- **Routes are the only handlers the network reaches.** A clock and a file have no caller. An
  action has callers, all through Holon, and its grant says which (§5).

The **old forms stay**, as the one-handler case, lowered as they are, their sinks' triggers open
(§3):

- `every 10s` with top-level statements is a flow with one clock handler, lowered as today: the
  whole canvas as one unit, its interval a deploy setting.
- `on file inbox` with top-level statements is likewise a flow with one file handler.
- A flow that reads `ingress` and has no other trigger is a flow with one unnamed action, run by
  `/trigger/<slug>` as today. Its capability stays `NetworkIngress`, so its hash and grant stay;
  the editor shows it as "can be run by hand" (§7).
- A flow with only routes lowers as today: a server of endpoints.

`fmt` keeps each flow in the form it was written. The new forms hash no old flow differently. The
sinks' trigger input (§3) does, once: every unit with a sink gets a new hash on its next deploy.

## 3. The language

```
item    = holon | route | every | onfile | action | statement | wiring
every   = "every" (duration | number) block
onfile  = "on" "file" ident ["deleted"] ["matching" string] "(" ident ")" block
action  = "action" ident "(" ident ")" block
```

- **A block form is a handler.** It is told from a header by what follows it: `every 2s {` opens a
  block, and `every 2s` at the end of a line is the old header. Likewise `on file inbox (f) {`
  and `on file inbox`. `action` has no header form: the old one is reading `ingress`.
- **Inside an `on file` block**, the name in parentheses is the file's handle: what `ingress` is
  in the old form, `{folder, name, size, hash, modified, event}`, with no `hash` for a deleted file (`folders.rs`
  488–500, `arrivals.rs` 337–356). `ingress` inside a block is an error
  that names the parameter.
- **An `every` block has no parameter.** `now` is the time of the tick, as in a clock flow.
- **An `action` block's parameter is what its caller sent** (§10, fork 11). It may `respond` a
  value, which is what the caller gets back: a person sees it under the run button, a flow gets
  it as the call's answer. Without `respond` the answer is empty. A status and headers are a
  route's, not an action's.
- **The old header and blocks don't mix.** In a flow with any handler block or route, an `every`
  or `on file` header is an error, and so is `ingress` outside a block, and a top-level statement
  other than a literal binding (§5.11 unchanged). The error says what to write instead: "put the
  clock's statements in an `every 10s { … }` block".
- **Two handlers with the same trigger are an error**, like two routes with the same shape:
  `every 2s` twice, the same `on file` line twice, or two actions of one name.
- **Several clocks are fine**, each its own period: `every 2s { … }` beside `every 1d { … }`.
- **Several file handlers on one folder are fine** when they differ, as several `on file` lines do
  today. A file matching two of them starts a run of each.
- **Several actions are fine**, each its own name: `reset`, `reindex`, `resend`.

### Membership: every sink takes its trigger

The rule stays: **a handler is everything wired to its trigger.** What changes is that every sink
is wired to one, as `respond` is to its request today (§1). The text and the graph then say the
same thing, and no node belongs to a handler by a field you can't see.

- **Every sink gets a `trigger: Any` input,** last, so positional calls don't move. These are the
  Effect leaves and the calls out listed in §1, seventeen of them. The value is not read; it only
  has to be there. Reads get none: a read's value matters only through what uses it, and that is
  tied. `channel_subscribe`, the fifteenth leaf with an Effect output (catalog.rs 351), is such a
  read: its Effect is a route's streamed answer, used by `respond`, so it gets none either.
- **A sink whose trigger is absent does nothing,** and core has to be made to do that. Eval hands
  a leaf only its present inputs (eval.rs 229), and `sink_effect` takes the first of them
  (primitives.rs 608–614). So with only a port added, an absent trigger would not stop
  `store_write`, `egress` or `channel_send`, and an absent value would write the trigger. A sink
  instead fires only when its trigger is present, and the trigger is dropped before `compute`.
  The model is the hold eval already has for a wired optional input whose value is absent
  (eval.rs 434–446).
- **The lowering wires it to the block's trigger:** `req` in a route, the tick in an `every` block,
  `f` in an `on file` block, `input` in an action. The text doesn't write it, as `respond x`
  doesn't write its request:

  ```
  route POST /hit (req) {
    state.n = (state.n ?? 0) + 1      # trigger: req
    respond 204
  }
  ```

- **`after` ties a sink to something else,** any value: `state.last = id after saved`, or
  `posts.put(k, v) after x`. It is not a node. It is how the text writes the trigger wire, and
  `fmt` prints it only when the trigger isn't the block's own. What it names is a statement of its
  own, before it, never written inline in the clause. An effect read only by a trigger is still
  performed: a trigger waits for it, it doesn't take it in (encapsulate.rs `default_spec`). Because an absent trigger means no
  write, `after found` is "write only if the lookup found something". `after` stays a name
  everywhere else (`list(after: …)`, `req.query.after`); it is a keyword only as the clause after
  a sink.
- **A tick's value is its time.** The `every` block's trigger node is a `now` read (the `clock`
  leaf), so sinks have something to tie to without the handler taking an input (fork 8 stands).
  As built in slice 2, it is the `every` template: the `clock` leaf with the period as its
  `seconds`, and `now` in the block reads its output. The printer writes its group back as the
  block, the ties left out.
- **A file's value is its handle.** As built in slice 3, an `on file` block's trigger node is the
  `on_file` template: the `file_arrival` leaf with the block's line as its `folder`, `on` and
  `matching`, its one output `file` what the parameter names. Its sinks are tied to it, and the
  printer writes its group back as the block.
- **The old forms stay untied** (as built in slice 1). A clock flow, a file flow, a by-hand flow
  and a flow of top-level statements are each one run of the whole canvas, so their sinks run
  with it through the open trigger below, as they did. Their ties come with the blocks of slices
  2–4, which replace them.
- **The wiring form ties nothing.** It is the graph as written: a tie there is a wire,
  `in.trigger -> w.trigger`, and a holon in it gets no trigger it doesn't declare. `fmt` writes a
  holon with an open sink inside (a canvas group) in that form, so it reads back untied.
- **A holon with a sink gets a `trigger` input,** last, whether the sink is in its body or in a
  holon it places. Its sinks are wired to it, and a placement wires it to the block's trigger, so
  `bump()` in a route is tied. `bump() after x` ties it to something else. It is the one port a
  holon has without declaring it, as `respond`'s request is (an exception to `LANGUAGE.md` §5.8).
  A holon with no sink, a calculation, gets none, and its outputs are as declared.
- **The tie is membership and presence, not order outside the run.** A handler still runs as one
  unit and commits as one. `after saved` waits for the put's result to exist in the run; it
  doesn't hold anything back from the host.
- **Two handlers still may not share a node.** A node wired to two triggers joins their groups,
  which is an error, as two requests wired together are today (routes.rs 81).

**What core does with it, as built.** The trigger is never read: a sink drops it before it
computes (`takes_trigger`, eval.rs and eager.rs), so it computes on its own inputs as it always
did. Wired and absent, it holds the sink back, as a wired optional input does (the hold, eval.rs).
Left unwired, it is open, and the sink runs with the unit around it, as an unwired optional input
lets a leaf fire. A holon's own trigger left unwired reads as present (`open_trigger`), so its
sinks run as a leaf's would. Open is what keeps a whole-canvas flow and a node just dropped on the
canvas working. It doesn't make membership invisible again: the lowering ties every sink a block
places, and the canvas flags an open trigger on a canvas that has a trigger to tie it to (§7).

Saved flows need a migration, because a saved flow is its graph. The editor saves a `FlowDocument`
of React Flow nodes and edges, with the text only as an optional `source` (`useSavedFlows.ts`
97–130, `server.ts` 750–776; the server keeps it as opaque JSON, `holon/src/flows.rs` 5–7). A
placed leaf's unit is rebuilt from the ports stored in its own node data, not from the catalog
(`holon-author/src/canvas.rs` 38–40, `flow.rs` 33–40), and nothing refreshes them on load
(`tabs.ts` 179–214). So on load (`tabFromDocument`: open, import, mode switch), an old sink gets
its `trigger` port added to its stored inputs, then the wire from its route's request when it is
in a route's group (`canvasOps.withTriggers`); one in no group stays open. Placing a saved flow as
a holon (`flowAsNode`) needs none: a sink saved without the port is not a sink with its trigger
to core (`takes_trigger` reads the last input's name), so it computes as it did. Samples are
compiled from text and need none. Deployments from before are not migrated: they are removed and
deployed again (Holon is in development).

## 4. The units

The flow's unit stays a composite of handler units with no edges and no ports, still
`declare=server`. A route-only flow is byte for byte the server it is today. Its children are:

- **an endpoint**, as today;
- **a clock handler:** `declare=every`, `seconds` (the period), no inputs, the handler's effects as
  outputs;
- **a file handler:** `declare=on_file`, `folder`, and `on` and `matching` when given. One input,
  `file: Record`, the handle; its effects as outputs. As built in slice 3 it has **no inputs**,
  as a clock handler has none: the handle is its `on_file` node's `file_arrival` leaf, sourced as
  `FileArrival(folder)` as the old form's is. So its capability and its watch come from the leaf
  (`file_triggers`), a waiting run keeps the handle among its sourced values (the widened filter,
  durable.rs 386), and verify reruns it with the logged handle, as it reruns a tick with its
  time. The three points below are what an input would have needed instead;
- **an action handler:** `declare=action`, `name`. One input, what was sent; its answer, then its
  effects, as outputs (the order an endpoint's are in).

`port` is there only when the flow has a route. A flow of only clocks, files and actions listens
on no port.

The trigger's value is the handler's **input**, as a request is an endpoint's. A file handler's
handle is an input, not a sourced `file_arrival` leaf, and an action's input is not a sourced
`NetworkIngress`. Three things follow:

- the capability comes from the declaration, as `declare=server` gives `NetworkIngress` today:
  `declare=every` gives `Clock`, `declare=on_file` gives `FileArrival(folder)`, and
  `declare=action` gives `Action(name)`;
- a waiting file or action run keeps its input, among the run's inputs. (For the old file form,
  the §1 gap is closed apart, by widening the parked run's filter: durable.rs 386.)
- `verify` reruns it as it reruns an endpoint: the unit, with the logged input. That needs the log
  to record it as the input. Today a file's handle and a `/trigger` body are logged among the
  sourced values (`FileArrival`, `NetworkIngress`), with `request` empty (arrivals.rs 371–381,
  deploy.rs 698–709).

`read_server` (`declare.rs` from 75) reads all four kinds. Its duplicate check (110–113, routes
of the same method and shape today) covers clocks (same `seconds`), file handlers (same `folder`,
`on` and `matching`) and actions (same `name`).

## 5. The host

- **Deploy.**
  - The capabilities are the union over the handlers, and the triggers come from the
    declarations.
  - `is_clock` becomes "has an `every` handler, or is an old clock flow".
  - Arrivals are watched from the `on_file` children as well as the old leaves. As built in
    slice 3: the `on_file` template's leaf is in the closure, so the watch is opened as before.
    A look fires, for a server, each file handler whose line is for the file, alone, logged as a
    `"file"` run under its label (`on file inbox`); the old form still fires the whole unit. A
    server lists its handlers' labels among its triggers in place of `Files inbox`, and refuses
    a handler's `FileArrival(folder)` marked optional.
  - "Declares no driver capability" stays, for a flow with no handler at all. Its message says
    "trigger" and what to write: "nothing would ever start a run: to run it by hand, put it in
    an `action run (input) { … }`" (fork 14). No handler is added for the author.
  - `interval_seconds` is accepted only for an old clock flow. For a flow with `every` blocks it
    is refused: "this flow's clocks are in its text".
- **The six `server.is_none()` tests** (§1, persistence) become "has an `every` handler, or is
  an old clock flow" for the clock, and "has an `on file` handler or `file_arrival` leaves" for
  arrivals.
- **The manifest names every trigger**, not one: `trigger()` (manifest.rs 45) becomes
  `triggers()`, and `permission_kind` (61) keeps each of them from being optional. The deploy
  dialog lists them as it lists what else a flow may do: "receive requests", "run every 2
  seconds", "run when a file settles in inbox", "can be run by hand: reset".
- **The clock** keeps a `next_due` per `every` handler, keyed by its period. A tick fires that
  handler's unit with no inputs, and logs a `"tick"` with the handler's label. The old form keeps
  its one `next_due` from the deploy setting. As built in slice 2: `Deployment.clock_due`, one per
  `server.clocks`, due at deploy, at load and when it runs again; the label is the entry's
  `route` and the history entry's `label`; a flow with clocks refuses `interval_seconds` and a
  `Clock` marked optional (the manifest still names one trigger: `NetworkIngress` beside routes);
  a server of clocks alone has no `port` and listens on none. **Tick now** is
  `POST /deployments/<slug>/tick` with `{"clock": "every 2s"}`.
- **Arrivals** drop the `server.is_none()` filter (arrivals.rs 299) and fire handlers, not the
  whole unit. A file fires each handler it matches with the handle as its input, logged as a
  `"file"` with the handler's label. The seen-set stays one per deployment, keyed by folder and
  name.
- **Actions** are run by `POST /trigger/<slug>/<name>` with what to send as the body.
  `/trigger/<slug>` stays, for the old form's one unnamed action, and answers 400 for a flow
  that has handlers. An action run is logged as a `"trigger"` with the label `action <name>`
  and, always, who ran it in `LogEntry.by`: a person, an agent, or `flow <slug>, run <id>`.
- **Who may run an action is its grant**, whoever asks. There are no access modifiers:
  - **a person:** an editor or an admin, as `/trigger` allows today (fork 13); a viewer may not;
  - **an agent:** only an action the deployment's grant lets agents run. The approval lists each
    action with "agents may run it", off unless a person ticks it, and changing it is a grant
    change, as an optional permission is;
  - **another flow:** only through its own grant (§9, slice 6);
  - **the network:** never. The data plane serves routes only.
- **Persistence** adds nothing new on disk: the handlers are re-derived from the unit at load, as
  the server is, and the agents' actions are part of the stored grant. Each `every` handler is due
  at once after a restart, as a clock flow is today.
- **`history`** (the last 20 ticks) gains the handler's label.

## 6. The log, replay and verify

- **`LogEntry.route` holds the handler's label for every handler run:** `"GET /live"`, `"every
  2s"`, `"on file inbox"`, `"action reset"`. The kind stays as it is. An old clock, file or
  by-hand flow's entries keep no label, and are read as today.
- **`LogEntry.by`** says who ran an action, as it says who sent a request through the control
  plane today.
- **`verify`'s branch is unchanged:** with a label, find the child with that label in the
  deployed unit and run it with the logged input (the request, the handle, what was sent, or
  none); without one, run the whole unit. The status check stays on `kind == "request"`
  (verify.rs 496–501), and every rerun keeps the one generous set of limits it has (468).
- **What a log withholds** (secrets, tokens, what is computed from a secret) is the same for a
  handler's run as for any run today: the label itself is the flow's text, never secret.
- **Replay** in the editor picks the trace by label, not by `kind === "request"` (replay.ts
  225–226, 247–250; the same test is at replay.ts 382 and 449, `useViewMode.tsx` 156 and
  `ViewPanes.tsx` 37).
- **A resume** delivers to any handler's waiting run. A clock handler that waits for approval can
  now be resumed by a route of the same flow, which a clock flow never could.

## 7. The editor

- **The canvas draws every trigger wire.** Each sink shows its `trigger` port, as `respond` shows
  its request. The wire to it is drawn lighter than a data wire, since many run from one trigger.
  Dragging it onto any node's output is how you draw an `after`, and `isValidConnection`
  (App.tsx 440–455) takes it like any `Any` input. A sink placed by hand is wired to the trigger
  when the canvas has only one. Otherwise it is flagged "not tied to a trigger", with a quick fix
  for each trigger on the canvas.
- **Frames are drawn, not dropped into.** Each handler's group, derived from its wires as
  `derive.ts` derives route groups today, gets a frame titled by its trigger (the canvas has no
  frames today, §1). Membership is the wires, so a frame is only a picture of them. The palette
  gains `every`, `on file` and `action` trigger nodes, beside `request`.
- **The ⚙ panel** shows "interval (s)" only for an old clock flow. For a flow with blocks, the
  periods are in the text and in the frames.
- **The handlers panel** (today's "server ▾" list of routes, `ServerPanel.tsx`) lists every
  handler: `GET /`, `GET /live`, `every 2s`, `on file inbox`, `action reset`.
  - A route row keeps its **test**, which loads the route into the request tester below the list,
    where **send local** and **send live** are (`ServerPanel.tsx` 160–166, 205–221).
  - A clock row gets **tick now**, which runs the handler once through the clock's own path,
    logged as a tick.
  - An action row gets **run**: a box for what to send (remembered per action), the answer under
    it, and **view run**, which opens view mode on the run it made. It runs the deployed version,
    and says so when the canvas has changes not deployed. A viewer sees the row, its button
    greyed: "an editor or an admin runs this".
  - Beside **run**, the command a script would use, to copy:
    `curl -X POST <server>/trigger/<slug>/reset -H "Authorization: Bearer $HOLON_TOKEN" -d '…'`.
- **The deployed page** lists a running deployment's actions as the same rows, so an operator
  runs them without opening the flow. Today's "request body" box becomes the row of the old
  form's unnamed action, labelled **run**.
- **A flow with no trigger** shows deploy greyed, "nothing would start a run", with a quick fix,
  **make it runnable by hand**. It wraps the flow's top-level statements in `action run (input) {
  … }` and, when the flow computes a value, ends the block with `respond` of it. The author can
  rename the action before deploying (fork 14).
- **The approval** (and `holon check`) show an old by-hand flow's `NetworkIngress` as "can be
  run by hand", not "receive requests" (`holon-author/src/describe.rs` 43, which both use): a
  label only, so no hash or grant changes.
- **The flows page** shows a flow's triggers beside its name: ":8129 · every 2s · 1 action". Today
  only its collapsed "may" section, opened, marks the permission that starts runs with a "trigger"
  badge (`Pages.tsx` 193–196, `Manifest.tsx` 53). The deployed page, which lists a deployment's triggers
  (`DeploymentCard.tsx` 109), lists every one.

## 8. The language docs and the manual

- `docs/LANGUAGE.md`:
  - §5.1: `every` and `on file` as headers (the one-handler form) and as blocks; `action`;
  - §5.11: every sink's trigger and `after`, beside "a route is everything wired to its request";
  - §5.8: a holon with a sink has a `trigger` port it doesn't declare;
  - §5.12: flows without routes, as the one-handler form;
  - a new tour entry, `ticker`.
- `docs/manual/reference/permissions.md` 31: a flow's triggers, not "one of them"; `Action` and
  who may run one.
- `docs/manual/language/reference.md` and `docs/manual/guides/files.md`: the blocks. Also drop
  "One per flow" (reference.md 58) and "One `on file` per flow" (files.md 159), which are already
  wrong: several lines work today (§1, the `file-audit` sample). That fix needn't wait for this
  design.
- A manual page on actions: running one from the editor, the deployed page and a script, and
  letting an agent run one.

## 9. Samples and slices

| slice | what lands | sample |
|---|---|---|
| 1. Every sink takes its trigger | the `trigger` input on the seventeen sinks (the catalog, `docs/manual/language/leaves.md` blessed by `crates/holon-author/tests/manual.rs`, the wasm parity checks in `holon-wasm/src/lib.rs`), the lowering's tie, `after`, a holon's `trigger` port, the migration of saved graphs, the canvas's trigger ports and quick fix | `counter`: a route that counts its hits with no wire to the request in its text, and records the last id `after` the put |
| 2. Clock blocks | `every … { }`, `declare=every`, deploy and the manifest's triggers, a `next_due` per handler, the log label, verify and replay by label, "tick now" | `ticker` without its action: a page that adds each tick as it arrives |
| 3. File blocks | `on file … (f) { }`, `declare=on_file`, arrivals for servers, the handle as an input | `drop-board` (new, beside today's `drop-box`): files dropped in a folder show on a live page (`on file drop (f)` sends on a channel, `GET /live` subscribes) |
| 4. Actions | `action … (x) { }`, `declare=action`, `Action(name)`, `/trigger/<slug>/<name>`, `LogEntry.by`, the agents' actions in the grant, the run rows in the handlers panel and on the deployed page, the no-trigger error naming `action run` and its quick fix | `ticker`'s `reset`, the flow at the top of this L4 |
| 5. The docs | LANGUAGE §5.1, §5.11, §5.12, the manual, the actions page | — |
| 6. Flow calls (later) | a flow runs another's action, by its grant | a shop that runs `billing`'s `charge` |

Each slice is one commit, with its sample, e2e test and live run, as `CLAUDE.md` asks. Slice 1
stands alone and is worth having even without the rest: it is what lets a route count its hits.
Slices 2, 3 and 4 build on it (their blocks' sinks tie to their triggers) and are independent of
each other.

**Slice 6, flow calls, is only a grant.** It needs no new kind of handler and no access model:

- the caller declares the call, as it declares a folder or a database today, and calls it
  (`billing.charge(x)`; the form is settled with the slice);
- its manifest lists "runs the `charge` action of `billing`", which its approval shows like any
  other permission, and the call is in its hash: pointing it elsewhere is a new version;
- approving the caller grants the call, with one rule: a person can grant a flow only what they
  could do themselves. An editor who may run `billing.charge` may let a flow do it; a viewer
  can't;
- `billing` logs an ordinary action run, `by` the calling flow and its run; the caller logs the
  answer as it logs an outbound request's, so `verify` replays it from the log instead of calling
  again;
- a call to a paused or stopped flow is an error the caller handles, as a failed request is.

## 10. Forks, surfaced

1. **What makes a node part of a handler.** *Decided in review (§11): wiring to its trigger, with
   a trigger input on every sink* (§3). Four options were weighed for the write that uses no
   input:
   - the block it was written in, as a `handler` field: the text says it, the graph doesn't;
   - an anchor wire drawn only while authoring and dropped from the unit: what deploys is not what
     you see;
   - an `after` node before the sink: honest, but a node on every such write. It is
     `x when is_truthy(req)` given a better name, and the trick drops a write on a falsy value;
   - a `trigger` input on every sink, as `respond` has: the wire is in the text's meaning, the
     graph and the unit.

   The cost is one change to seventeen leaves, and a new hash for every unit with a sink on its
   next deploy (§2).
2. **The flow's unit.** `declare=server` gains clock, file and action children, with `port` only
   when there's a route, or a new `declare=flow`. *Pick: keep `server`.* A route-only flow stays
   byte for byte what it is, and the host reads one shape. The cost is a name that says less than
   it holds: a flow with only an `every` block is a "server" with no port.
3. **Where a clock's period lives.** In the unit (the hash), or in the deploy settings as today.
   *Pick: the unit, for blocks.* Several clocks can't share one setting, and a period then shows
   in the deploy dialog, the log and verify: changing it is a new version you approve. The old
   form keeps its setting.
4. **How a trigger's value reaches the handler.** As an input, as a request is, or as a sourced
   leaf, as `file_arrival` and `ingress` are. *Pick: an input.* It gives the capability by
   declaration and lets verify rerun every handler the same way. (Keeping a waiting file run's
   handle is not a reason: the leaf form gets that from a one-line filter, §1.)
5. **The old forms.** Keep the headers and `ingress` as the one-handler case, or rewrite them to
   blocks. *Pick: keep them.* Their units keep their shape (the sinks' trigger aside, §3), and fmt keeps each form. A
   flow that grows a second handler moves to blocks, and the error says how.
6. **The log's label.** Reuse `LogEntry.route` for every handler's label, or add a `handler`
   field. *Pick: reuse `route`.* Verify and the editor's trace already key on it, and an old log
   reads the same. The cost is a field whose name says "route" for a tick; the docs say "the
   handler's label".
7. **Same trigger twice.** Refuse two `every 2s` blocks (or two identical `on file` lines, or two
   actions of one name), or allow it. *Pick: refuse,* as two routes with the same shape are; one
   block can do both things.
8. **A tick's input.** None (`now` is the time), or a record `{at, due}` with the scheduled time.
   *Pick: none, for now.* Nothing needs the scheduled time yet, and adding an input later is a new
   unit only for flows that use it.
9. **Running a handler by hand.** *Decided in review (§11):* actions are the handlers run by hand,
   through `/trigger/<slug>/<name>`; a clock is tried with "tick now", through the clock's own
   path.
10. **A file matching two handlers.** One run per matching handler, or only the first. *Pick: one
    run each,* as two subscribers to a folder would expect. The seen-set still records the file
    once.
11. **An action's input.** A value (the body read as JSON), or text, as `ingress` is today.
    *Pick: a value.* A flow calling an action (slice 6) sends a value, and a person's box and a
    script's body are JSON; a text is a JSON string. The old form keeps text.
12. **Where an action is run.** `/trigger/<slug>/<name>`, or a new
    `/deployments/<slug>/actions/<name>`. *Pick: `/trigger`.* The old form's `/trigger/<slug>` is
    then simply the unnamed action, and the access rule for it (§1) is the one to extend.
13. **Which people may run an action.** Editors and admins, as `/trigger` today, or admins only.
    *Pick: editors and admins.* Editors deploy, and an action is code they could change; a flow
    whose actions only admins should run is a case for a per-action rule later, not the default.
14. **A flow with no trigger.** Bind it to a generic action so it deploys, or keep refusing it.
    *Decided in review (§11): keep refusing it, and say what to write.*
    - An action that nobody wrote would give the flow "can be run by hand" in its approval
      without anyone choosing it. That is the hidden membership fork 1 removed.
    - A flow with no trigger is a holon, made to be placed in other flows (`flowAsNode`,
      `canvasOps.ts` 222), and binding it would make every such flow look runnable.
    - A pure one, run with nothing, computes the same value each time. Running it is worth
      something only when it reads or writes, and then it is an action, written as one:
      `action run (input) { respond summarize(input) }`.

    The deploy error names that form and the editor offers it as a quick fix (§5, §7). It is
    not called `tick`, which is a clock's word. A file of only holon definitions, with nothing
    placed, is refused today as "the canvas is empty" (`holon-author/src/flow.rs` 159). A library
    of holons is outside this L4.

## 11. Decided in review

The first review (2026-10-26) settled:

- **The words:** trigger, handler, route, clock, file, action (the table at the top). "Driver" is
  not used for what starts a run.
- **A run by hand is an action,** a fourth kind of handler, not a route with a modifier: a route
  is open to the network by what it is, an action never is.
- **No access modifiers** (`public`, `internal`, `private`). Who may run an action is its grant,
  whoever asks: people by role, agents as a person granted, other flows by their own grant.
- **A flow calling another is a grant** on the caller, approved by someone who could run that
  action themselves (§9, slice 6). It comes after the first slices.
- **A flow still needs at least one trigger.** A flow without any is a holon, placed in other
  flows; deploy keeps refusing it (§1). On 2026-10-27: no action is bound for the author. The
  error and a quick fix offer `action run (input) { … }` (fork 14).
- **Every sink takes its trigger** (fork 1, §3), on 2026-10-27: a `trigger` input on every sink,
  wired by the lowering, written `after x` when it is something else. A holon with a sink gets the
  same port. Membership stays "everything wired to its trigger".

## 12. What the second check corrected

Checked again at `1ccf376`, after rc.25 to rc.27 (security fixes, secret-derived values, databases,
commit and run locks). The design's ground holds: nothing since lets a flow have two triggers, and
the three behaviours it starts from were run again with `holon check`. What changed in the text:

- **The run budgets were misstated.** Triggers and resumed runs get the request budget; only ticks
  and file runs get the clock budget (§1). The design's rule for handlers is unchanged.
- **"The `likes` sample's case" was wrong:** `likes` has only routes. No sample yet has two kinds
  of run on one state (§2).
- **`in` is not a clause on a `node` line;** it is a reserved name. (The `for` clause proposed
  in its place went with the `handler` field, when fork 1 was decided for wiring.)
- **The verify "fix" is dropped.** Every rerun gets one generous set of limits on purpose (§6).
- **A waiting file run's handle** can be kept in the old form by widening one filter, so it is no
  longer an argument for fork 4 (§1, §10).
- **Added:** the run log's `"file"` and `"resume"` kinds, rc.26's withholding in logs, rc.27's
  `database` header and locks, the six `server.is_none()` tests, the editor's lack of frames and
  of a per-node handler field, and the two manual lines that already contradict the code (§8).

## 13. What the third check corrected

Checked again at `d4223a8` (`origin/main` at `1ccf376`, plus one commit to `DeployBar.tsx`), after
fork 1 was decided. Every section was read against the code. What changed in the text:

- **The sink trigger needs a change in core,** not only a port (§3). Eval hands a leaf only its
  present inputs and `sink_effect` takes the first, so an added port alone would not stop a write
  whose trigger is absent, and would write the trigger when the value is absent.
- **The reason the trigger can't be optional was the wrong way round** (§3). An unwired optional
  input reads as "none" and the leaf fires; only a wired, absent one holds it back.
- **Fifteen leaves have an Effect output,** not fourteen: `channel_subscribe` is a read and gets no
  trigger (§1, §3).
- **The old file form's tie** is the `coalesce` chain of its `file_arrival` leaves, not one read;
  a clock flow has a `clock` leaf only when it writes `now` (§3).
- **Saved flows are graphs,** whose leaves carry their own ports, so the migration adds the
  `trigger` port as well as the wire, at `tabFromDocument` and `flowAsNode` (§3).
- **Today's facts corrected:**
  - the file handle's fields (§3);
  - `likes` bumps a collection item (§2);
  - six `server.is_none()` tests, not four (§1, §5);
  - `/trigger` fires any flow but a server (§1);
  - a route row has only **test** (§7);
  - the log records a file's handle and a trigger's body as sourced values, not as inputs (§4, §6);
  - the counter example and the holon example, which now check as written (§1);
  - a few line numbers: parse.rs 286, lower.rs 337, recorder.rs 66, main.rs 364, ServerPanel.tsx 160–166.
