# Triggers and clocks

What starts a run: a request on the flow's port, a clock, a file settling in a folder, a message
in a mailbox, an event from a feed, an action someone runs, another flow's call, and a resume. A flow is its imports and settings, its functions
and its **handlers**, each a trigger and the function it runs; they share everything the flow owns.
The design is the [triggers L4](../../L4s/triggers-L4.md) (TL4); the language is
[`LANGUAGE.md`](../../LANGUAGE.md) §5.1 and §5.11; how each kind of run is recorded and synced
is [Performance](../themes/performance.md#three-levels-of-record) and
[Resilience](../themes/resilience.md) G1, G2 and G8.

## 1. What it is

| Handler | Trigger | Its runs | Capability | Code |
|---|---|---|---|---|
| `route.get("/notes", (req) => …)` | a request on the flow's port | one a request, `req` the request | `NetworkIngress` | `listeners.rs`, `dataplane.rs` |
| `every("2s", () => { … })` | the clock | one a period | `Clock` | `scheduler.rs` |
| `on.file.created(inbox, (f) => { … }, { matching: "…" })`, `on.file.deleted(…)` | a file settling in, or leaving, a named folder | one a file, `f` its handle | `FileArrival("inbox")` | `arrivals.rs` |
| `on.mail(remit, (m) => { … })` | a message in a bound mailbox | one a message, `m` the message read | `MailArrival("remit")` | `inbox.rs` |
| `on.feed(prices, (e) => { … })` | an event from a bound feed of server-sent events | one an event, in order, `e` the event | `FeedEvent("prices")` | `subscriptions.rs` |
| `export const reset = action((input) => { … })` | a person, an agent or another flow, through Holon | one a call, `input` what was sent | `Action("reset")` | `deploy.rs` |

Every flow has handlers. The forms without them (`every 10s` or `on file inbox` as a header, the
whole flow its unit; a flow reading `ingress` and nothing else) went with the switch to TypeScript
(LANGUAGE.md §10), and the server, the `ingress` leaf and `holon trigger` with them. A **resume** route
(`{ resumes: "paid" }`) continues a waiting run ([Durable runs](durable-runs.md)). A **channel**
starts nothing: a send is a sink, and a subscription is a route's run.

Samples: `hello-api` (routes), `heartbeat` (a clock alone), `rolling-log` (a clock that keeps 5
runs), `ticker` (a clock beside routes, and an action), `drop-box` (a file handler alone),
`file-audit` (several file handlers, removals, `matching`), `drop-board` (file handlers beside
routes), `remittance-mail` (a mail handler beside routes), `support-inbox` (a mail handler that
answers through the same mail, logged in with OAuth), `price-feed` (a feed handler beside routes,
telling a page through a channel), `billing` and `shop` (actions, and a
flow calling another's), `approval` (a resume route), `status-board` (each handler's log level).

## 2. How it works

### What deploy reads

Deploy derives the triggers from the unit: a server's `routes`, `clocks`, `files` and `actions`
(`read_server`, `declare.rs`: a `Server`). A unit that isn't a server, or one with no handler,
would start nothing, and is refused with what to write (`STARTS_NOTHING`); a saved deployment of
one is skipped at load, saying so.
Each handler has a label core gives it (`GET /notes`, `every 2s`, `on file inbox`,
`action reset`), the label is the log entry's `route`, and a trigger is never an optional
permission (`permission_kind`). A clock's period is at most `MAX_PERIOD` (366 days).

### Routes: a port per deployment

A server deployment with a `port` gets a listener of its own on `--data-host` (default
`127.0.0.1`), started and stopped by `sync_listeners` as deployments come, go, stop or change port
(`dataplane::start_listener`: hyper on a shared tokio runtime, at most `max_connections`). A port
another program holds is said on the deployment (`listen_error`). `serve_flow` matches the request
to a route (`match_route`), answers 503 for a deployment not running, and fires the endpoint with
the request record, the clock and random numbers sourced. Whether it runs on the worker or the
blocking pool is `runs_here_for` ([G2](../themes/resilience.md#g2-a-deployment-cant-slow-another)).
`POST /deployments/<slug>/request` (the editor's "send live") goes through the same `serve_data`
over the control plane.

### Clocks

`spawn_clock_scheduler` starts one thread that, every second, calls `run_due_clock_ticks` and then
`expire_waiting`. A clock of a running deployment is due when its `clock_due` (one per `every`
handler, `server.clocks`) has passed; every due tick of the pass fires on its own scoped thread,
and the pass ends when all have. `fire_one_clock_tick` fires the handler with no inputs, `Clock` (the tick's epoch seconds) and `Random`
sourced and the clock budget, logs a `tick` under the handler's label, keeps the last 20 ticks
(`history`), and sets the next due a period after the pass's `now`. `arm_clocks` makes every clock
due at once: at deploy, at load, and when a deployment runs again. **Tick now** is
`POST /deployments/<slug>/tick` with `{"clock": "every 2s"}` (`handle_post_tick`, `holon tick`).

### Files

`arrivals::spawn_watcher` looks, every `POLL` (1 s), at the folders running deployments watch that
the OS says changed or that hold files still settling, and at all of them every `WALK` (30 s). A
file new or changed since the deployment saw it starts a run once it stops changing (the same size
and modified time at two looks in a row), its handle (`folder`, `name`, `size`, `hash`, `modified`,
`event`) the run's input; a removed file starts an `on.file.deleted` handler's run with its handle
as last seen. What a deployment has seen is kept beside its log, so files that arrive while the
server is down or the deployment paused start their runs when it is back; files there at deploy
are seen, not run on; a file the flow's own runs wrote, copied or deleted starts nothing. A look
starts at most `MAX_RUNS_PER_LOOK` (100) runs a deployment. A server's file handlers each run
alone, a file two handlers are for starting a run of each, logged as `file` under the label.

### Mail

`inbox.rs` is the same for a mail handler (`on.mail(remit, (m) => …)`, host-capabilities L4
§5.2.4): a thread looks every second for the mailboxes running deployments listen to whose
binding's `--every` has come round, logs in over IMAP (`imap.rs`; with a password, or with the
OAuth token `oauth.rs` asks for and keeps, through `Mails::open_mailbox`), and starts one run for each
message that came since, 20 a look at most, its message read by core (`mail::received`) the run's
input as `MailArrival("remit")`. Once the run is over the message is moved to the binding's
`--done`; its UID is noted first, so a move a crash cut off is made at the next look. What the
folder holds at the first look is seen, not run on, and a second copy of a message is moved without
a run. What each deployment has seen is in `mail-seen/<slug>/<mail>.json` beside its log; a look
that fails is on the timeline once, and again when the mailbox reads again. Logged as `mail` under
the label.

### Feeds

`subscriptions.rs` holds the connections a feed handler needs (`on.feed(prices, (e) => …)`,
host-capabilities L4 §5.9.2), for every kind of feed to come: every second a thread compares the
feeds running deployments listen to with the connections it holds, and opens or closes them, one
a deployment and feed, opened anew when the deployment's unit or the feed's binding changes. Each
has a reading thread, which reads the feed's events (`sse.rs`, a client over `rustls` the host
can close from another thread) into a buffer of the binding's `--buffer`, and waits while it is
full; and a running thread, which runs the handler on each, one at a time, its event the run's
input as `FeedEvent("prices")`: `{data, event, id}`, `data` parsed when it is JSON. A run lock
per deployment and feed outlives a connection, so a new version waits for the old one's run and
reads where the feed is after it: no event runs twice. Where the feed is (the last id run on, and
the last 1,000) is in `feed-seen/<slug>/<feed>.json` beside the log, written at each run's end; a
reconnect sends it as `Last-Event-ID`, and an event sent again by its id is passed over, on the
timeline. A connection lost is opened again after a second, or the feed's `retry:`; one that
can't be is tried again after twice as long each time, to a minute, on the timeline once. What
each connection is doing is in `GET /deployments/<slug>/feeds`. Logged as `feed` under the label
`on feed prices`.

### Actions and flow calls

`POST /trigger/<slug>/<name>` (`handle_post_action`) runs one action with a JSON body (nothing is
`{}`), logged as a `trigger` under `action <name>` with `by` saying who ran it; it answers
`{answer, seq}`. An editor or admin may run it; an agent only with `AgentRuns("<name>")` in the
grant (403 otherwise); the network never. `POST /trigger/<slug>`, naming no action, answers 400
naming the way in (`unnamed_action`), or 404 for a deployment with no action. Another flow runs an action through its own grant (`RunsAction("billing/charge")`) with
`flow_call`, performed between rounds like an outbound call, its answer kept in the caller's record
so a retry and `holon verify` never call again; a chain is at most `max_call_depth` deep (3 by
default, 8 at most) and answers `too_deep` past it.

### How each run is recorded

A handler's options may say `{ log: "lazy" }` or `{ log: "unrecorded" }` (the frozen `log` param;
strict when absent):

| Run | Level |
|---|---|
| a route, an `every` handler, a file, mail or feed handler, an action | as its `log` option says (`Permit::log_level`) |
| a request no route took | `lazy` (Q12) |
| a resumed part | strict |

Any handler may be `lazy` or `unrecorded`, as its author writes it, whatever it does. `unrecorded` holds only where
granted (`Unrecorded("GET /health")`, and `[grants] allow_unrecorded`); ungranted, the run is
strict. A strict run answers once its entry is synced; a lazy one once it is written; an unrecorded
one is counted, not logged (`count_if_unrecorded`). A run nobody waits for (a tick, a file run)
syncs the same way but has no one to answer.

## 3. Why it is this way

- **Handlers, not one trigger a flow.** The trigger shouldn't decide what a flow can be: a clock
  beside routes shares the routes' state, channels and grant, one deployment, one log, one chain,
  one approval (TL4 §2). At rc.29 the forms without handlers lowered as before, every sink gaining
  a `trigger` input; with the switch to TypeScript every flow has handlers, units got new hashes
  (Holon keeps nothing compatible while in development), and the server stopped running a unit
  without them.
- **A port per deployment.** A deployment is a server of its own; one slow or busy deployment's
  listener is not another's, and stopping it releases its port (lifecycle).
- **Actions never from the network.** A route is the network's way in; an action is Holon's, so who
  may run it is its grant: a person, an agent ticked one by one, another flow through its own
  approval (TL4 §5).
- **Ticks of one handler never overlap**: the next is due a period after the pass that ran it, not
  before it ends (TL4 §2).
- **Due at once after a restart**: nothing about a clock is persisted but the deployment; a tick
  missed while the server was down is not made up, the next is now.
- **A file runs once it settles**: a file read while being written would be read half; two looks
  alike is the cheapest test that needs no cooperation from the writer.
- **Strict by default; lazy and unrecorded where the author says.** Whether a run's record is
  worth its cost is the flow author's call, not the host's: a handler that writes, calls out,
  sends or waits may be lighter too, and what relies on the record says what it then can't do
  (`holon verify` leaves the cells they write unchecked, a restore's dry run names the unrecorded handlers,
  their packs go out live, the other flow's log says the run isn't logged). It was refused once
  (C5, `b1259e3`). Unrouted requests are lazy, because strict
  would let anyone cause an fsync a request (Q12).

## 4. Reach and limits

- **A clock pass waits for its slowest tick.** `run_due_clock_ticks` joins every tick of a pass
  before the next pass begins, so one slow tick delays every other deployment's next tick, and
  expiry, until it ends. Each due tick is a thread of its own, unbounded (the code names it: a
  bounded pool is the fix if fan-out grows).
- **Clocks drift and skip.** The next tick is due a period after the pass's `now`, checked once a
  second; periods are whole seconds; a tick late by more than a period is one tick, not several.
- **Files are polled**, every second where the OS gives notices and every 30 s regardless: a dropped
  notice costs up to 30 s; a network share is looked at every second. A file that never stops
  changing never runs; a burst past 100 waits for the next looks.
- **One port per deployment**, all on one `--data-host`; a port taken by another program leaves
  the deployment deployed but not answering.
- **Resume routes on a paused deployment answer 503**, though its waiting runs still expire.
- **A lazy run may be lost in the last second before a power loss**; an unrecorded one leaves only
  its tally, at most a second of counts lost on a restart ([G8](../themes/resilience.md#g8-an-unrecorded-run-leaves-no-entry-failed-or-not-and-is-counted)).
- **Actions answer the run's respond, not a stream**; a waiting action answers `waiting` to a
  calling flow.
- **No cron**: `every` takes a period, not a time of day.

## 5. What the tests hold

In `crates/holon/src/tests.rs`:

- Routes: `a_server_deployment_answers_on_its_own_port_within_limits`,
  `a_server_deployment_whose_port_is_taken_says_it_is_not_answering`,
  `listening_beyond_loopback_warns_that_it_is_plain_http`,
  `a_light_route_answers_on_the_data_planes_worker_as_any_route_does` (G2).
- Clocks: `clock_ticks_fire_when_due_and_not_before`,
  `multiple_due_clock_deployments_all_fire_in_one_pass`, `clock_tick_with_state_persists_across_ticks`,
  `a_clock_runs_history_never_shows_a_secret`, `delete_deployment_stops_future_clock_ticks`,
  `a_servers_clock_ticks_on_its_period_beside_its_routes_and_verifies`.
- Files: `new_files_start_runs_that_send_them_on_by_handle`,
  `on_file_lines_run_on_several_folders_deletions_and_matching_names`,
  `a_file_a_run_deletes_starts_no_run`,
  `a_servers_file_handlers_run_alone_for_their_lines_beside_its_routes_and_verify`.
- Actions: `a_servers_actions_run_by_name_with_what_was_sent_and_verify`,
  `trigger_with_state_persists_across_calls`, `an_actions_run_is_given_the_clock_and_randomness`,
  `delete_deployment_stops_its_actions_and_removes_it_from_the_list`,
  `an_actions_reported_entry_is_its_own_under_concurrent_runs`,
  `a_flow_runs_another_flows_action_and_both_logs_say_so`, `a_call_that_cant_be_made_says_why`,
  `a_chain_of_calls_is_never_a_loop_and_is_only_so_long`.
- Levels: `a_strict_run_is_on_disk_before_its_answer_and_a_lazy_one_waits_for_the_syncer`,
  `an_unrecorded_run_leaves_no_entry_failed_or_not_and_is_counted`,
  `a_policy_without_allow_unrecorded_refuses_the_deploy`; in `holon-core`'s `declare.rs`, a handler
  that writes refused as `lazy`.

End to end (`editor/e2e/samples.e2e.ts`): every sample deploys; `ticker's clock ticks beside its
routes…` (tick now, the action from the editor and the deployed page, each run replayed);
`shop runs billing's actions…`; `drop-board's file handlers run beside its routes…`; `drop-box
runs once for a file dropped in its folder…`; `file-audit runs on a csv dropped in the inbox…`;
`a flow nothing would start can't deploy; given an action, it deploys and runs by hand`.

**Gaps.** Nothing tests a slow tick holding up other deployments' ticks, a burst of more than 100
files, or a folder whose OS notices are dropped.

## 6. History

| Release or commit | Change |
|---|---|
| before rc.15 | A deployment by slug (`ingress-deploy-L4`), the firing loop (`P2.4.1`), a clock flow on `interval_seconds` |
| rc.22 | `on file` runs a flow per file once it settles; `every` makes the clock a flow's trigger |
| `6bc5c68` | rc.29: several handlers in a flow (`every`, `on file`, `action` beside routes), every sink tied to its trigger, actions by name, flows calling each other's actions |
| `9ba3eb1` | A clock's, file's or action's run placed by its label (holon-author's trace, as the editor's replay does) |
| `f06e7a1`, `b1259e3` | `lazy` and `unrecorded` on a handler's line, checked against what it does; strict by default |
| `e40e7b5` | `unrecorded` handlers, approved per handler, counted |
| `c5e5556`, `dc21f76` | A short route answered on the data plane's worker (12.1); narrowed to runs that can never wait (R2) |
| `4bb40a8` | An unrecorded run is never logged, failed or not |
| TypeScript L4, step 7 | Flows without handlers gone from the server: no `interval_seconds`, no `ingress` leaf, no `holon trigger` or unnamed `/trigger/<slug>` run |
