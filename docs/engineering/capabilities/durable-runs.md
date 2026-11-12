# Durable runs

A run that waits for an event, and continues when it arrives, minutes or days later: an order that
waits for its payment, a payout that waits for its approval. The design is
[`PLATFORM_LAYOUT.md` §4.3 and §4.4](../../PLATFORM_LAYOUT.md); the language is
[`LANGUAGE.md`](../../LANGUAGE.md), "approval: a durable run"; the samples are `approval` and
`approved-payout`.

## 1. What it is

An action writes `const paid = await event("paid", { expires: "1d" })`; a route starts it by
calling it without `await`, `const started = payment(req.json)`, and answers with the token it is
handed back, `started.waiting.paid` (a route answers and is done, so it never waits itself). A
route with the option `{ resumes: "paid" }` continues the run:
`route.get("/paid/:token", () => …, { resumes: "paid" })`. A run that settles still waiting at an
`await` performs nothing, its starter gets the tokens of the points it waits at, and the host
keeps a parked run: what it needs to run again, with the event, from where it began. A resumed
part's log entry keeps its tokens as `‹token:NAME›`, as the starter's does.

In the code:

- `crates/holon-core/src/catalog.rs`: the `await` leaf (params `name`, `expires`, default 604,800
  s) and `resume_token`; `primitives.rs`: `MAX_AWAIT_SECONDS` (90 days).
- `crates/holon-host/src/rounds.rs`: the round loop's `Waits` and `Asking`; a run still waiting is
  `RunOutcome::waiting` and performs nothing.
- `crates/holon/src/durable.rs`: parking (`park_if_waiting`, `ParkedRun`), tokens (`token_for`,
  `load_host_key`), claiming (`claim_by_token`, `Claimed`), resuming (`resume_parked`), expiring
  (`expire_waiting`), `GET /deployments/<slug>/runs` (`handle_get_runs`).
- `crates/holon/src/lifecycle.rs`: the deployment's lifecycle and sessions, which a waiting run
  outlives.

## 2. How it works

### Waiting is a third state

An `await` with no event, or a request not yet answered, is **pending**, not absent
([lazy choices §4](../../L4s/lazy-choices-L4.md)): `Asking` answers `NotYet`, a `guard` or
`coalesce` whose deciding input is pending waits rather than decides, and an `await` in a branch
not taken never parks the run. In a run's final round nothing is pending unless the run waits, and
then nothing is performed. The packs it settled that don't wait are the exception: they go out on
their channels as it parks, logged as the part's `progress`, and a later part sends only what an
earlier one didn't ([channels §10](../../L4s/channels-L4.md)).

### Park

Every way a run starts calls `park_if_waiting` after it fires: a route (`listeners.rs`), a clock
(`scheduler.rs`), a file (`arrivals.rs`), `/trigger` and an action (`deploy.rs`). A run that
settled waiting and didn't fail becomes a `ParkedRun` in the deployment's `parked` map: its run
id, the unit it ran, its kind and route, its inputs and sourced values, the state it read by
reference (`state_refs`), its nonce, every round of requests it made (`rounds`), the events
delivered so far, each waiting point with its deadline, and the `version` it started on. Each
deadline is the `await`'s `expires`, cut to the grant's `max_await_seconds`. A deployment already
holding `max_waiting_runs` refuses another: the route answers 503 and the entry says why. The log
entry names the points it waits at (`LogEntry::parked`); the deployments file is written again
(`persist_deployments`).

A parked run keeps what it waits with in its own record (`ParkedRun`), not in the log, so a lazy
or unrecorded handler waits as a strict one does: the record keeps its handler's level
(`ParkedRun::log`), and each resumed part is logged at it, unrecorded where granted. What it sent
as it parked is kept there too (`ParkedRun::sent`), so after a restart a later part doesn't send
it again, though an unrecorded run's log doesn't hold it.

### Tokens

A token is derived, not stored: `token_for` is the first 128 bits of BLAKE3 keyed with the host's
32-byte key over the run id and the waiting point, `rt_<32 hex>`. The same run and point give the
same token every time, so a resumed run hands it out again with nothing kept, and no run record,
log or copy of the deployments file holds anything that continues a run. A presented token is
compared in constant time (`same_token`). The key is a file of its own, owner-only (`--host-key`,
`host.key` in the data folder; made at first start by `load_host_key`). Runs parked before tokens
were derived keep their random ones (`ParkedRun::tokens`), still accepted.

### Resume

A request on a resume route carries the token as a path param or `?token=`. Before its handler
runs, `claim_by_token` finds the run waiting at that point for that token and claims it under the
deployments lock (`resuming`), so a second request carrying it at once finds nothing; none found
is a 404 ("no run is waiting here for that token"). The route's own handler then runs and is
logged; then `resume_parked` delivers the request record as the event: it opens the record, makes
a `Resumption` (the same unit, inputs, sourced values, nonce and starting state, its earlier
requests answered from the record), and fires it with `fire_with_state`. The part is logged as a
`resume` entry under the same run id, on the version it started on. It finishes (its effects
performed now, once; it leaves `parked`), fails (it leaves too), waits again (its rounds and events
kept, a point it already waited at keeping its deadline), or is turned away by a run lock (`busy:`)
and waits on, its event to be delivered again. Dropping the claim frees a run still waiting. The
resume route's caller gets its answer once the resumed part has run.

A resumed run reads the state as it was when the run started, and its writes apply when it
finishes, over what is there then. It checks what it shares at its commit as any run does: if a row
or file it read changed meanwhile, it runs again from where it last waited, every request and
event answered from its record (`approved-payout`). A waiting run holds no claims
(`a_parked_run_releases_its_claims`), so a deposit can go ahead while a payout waits.

### Decide

The target design is [`access-and-approvals-L4.md`](../../L4s/access-and-approvals-L4.md) §8:
approvals in steps (slots filled by roles, in order, separation of duties, the starter never
deciding), principals and a permission catalogue in place of the four roles; it generalises what
follows, `people: N` staying one step's short form. Read it before changing this.

An `approval` is a waiting point people decide, never a token. Its request is an await's with
`people` (1 to 5), kept on the waiting point (`StoredWaiting::people`; 0, not written, for an
event). `waiting_point_of` matches no approval, so no token resumes one; a start hands none out for
it (deploy.rs, `waiting_at`); and a route `{ resumes }` or a `resume_token` naming one is refused
when the flow compiles (flow.rs).

`POST /deployments/<slug>/runs/<run_id>/decide` takes `{name, approve, reason?, slot?, note?}`.
`decider` says who may decide at all: a principal signed in, by their name (the run file as the
account that set the server up, so the same person counts once, D45), holding `run.decide` at the
approval point; never a service (a script, not a person), the run file tied to no account, or
anyone on a server without accounts. A `Decider` (who, kind, and `holds`: what they hold a role through there,
`can::holding`) then takes a step: the one named, or the one open step they may fill, of their
kind, bound to its role, never of a run they started, once per principal. `decide` runs under the
deployments lock. Each decision is kept on the waiting point (`decisions`: its step, who, their
kind and role, when, approved or not, why, the note, and what let them: `binding`, the binding's
id, and `access`, the access document's version it was read in, D43: every decision has them,
as `can::holding` finds only what a binding of the document holds). Once as many as it asks have approved, or anyone declines (a decline needs a reason),
the run is claimed, as a token claims it, and `resume_parked` gives it the decision as its event:
`{approved, by, decisions}`, with `declined_by` and `reason` for a decline. The event is made from
the decisions kept (`StoredWaiting::take`), so a decline a run lock turned away is still the
decision when someone decides again. So the resumed part's log entry, and what `holon verify` and a
replay read, hold who decided. Each decision is in the activity log too (`approved`, `declined`).

People decide in the editor's Approvals (Runs waiting) or with `holon decide <slug> <run> <name>
--approve|--decline --reason …` (decidecmd.rs), signed in through the run file as the account
that set the server up; `holon
decide <slug>` and `holon deployments <slug>` list what runs wait at, with who has approved so far
(`decidecmd::waiting_lines`), and the steps the caller may take (`yours_lines`). `holon mcp`'s `runs`
adds the same lines for an agent to tell the user, and its `decide` takes a step written for an
agent through the same route, as the agent; for a person's step it posts nothing, and says who
decides it and where. An agent bound to no role that holds `run.decide` there is refused before
the route.

### Expire

The clock scheduler's thread, every second, calls `expire_waiting`: each waiting point past its
deadline is claimed (one being resumed is left alone) and given `{expired: true}`
(`expired_event`), so the flow decides what timing out means. An approval is given its decision
as it stands, `approved` false, with `expired: true` and who had approved.

### What survives a restart

- **Waiting runs**, in the deployments file. Their inputs, sourced values, requests, events and any
  old tokens may hold a secret or a caller's credential, so the file keeps them sealed
  (`sealed_for_disk`: AES-256-GCM under the secrets master key, bound to the deployment and run
  id, `parked_context`), and the server opens them as it starts (`ParkedRun::open`). A server whose
  secrets live in memory has no key to keep, and seals nothing.
- **Tokens**, because the host key is a file: losing or replacing it invalidates every outstanding
  token.
- **Not the claim**: `resuming` is never persisted; a restart frees it.
- **Sessions**: a deployment is running, paused (answers 503, skips ticks, keeps its session) or
  stopped (the session ends, its state kept as the session's snapshot by reference); running again
  starts a session from the latest state, from nothing, or from an earlier session's snapshot
  (`Lifecycle::run`, `RunFrom`). A waiting run is the deployment's, not the session's.
- **Each part's log entry**, as any run's ([Run records](run-records-and-verify.md)).

`GET /deployments/<slug>/runs` lists the waiting runs (`summary`: run id, kind, route, when it
parked, its waiting points and deadlines, an approval's `people` and `approved`, the points
delivered), never their tokens; the editor's
deployment card shows how many wait.

## 3. Why it is this way

- **The run continues; the author doesn't hand-roll state** (insert the context under an id, look
  it up in the callback): minted ids, pinned versions, expiry, visibility (`PLATFORM_LAYOUT.md` §4.3).
- **Performs nothing while waiting.** A run's effects are decided when it finishes, once; a waiting
  run that wrote would be a half-run. Progress packs are the one exception, because a parked run's
  end may be days away ([channels §9](../../L4s/channels-L4.md)).
- **Replays from its record.** Evaluation is pure, so the inputs, nonce, state read and the answers
  it already had are enough to run it again to where it waited; the record is also what `holon
  verify` checks.
- **A token apart from the run id.** The run id hashes the run's inputs, computable by anyone who
  knows the flow and its state, so it can't be the secret; and one id could not be scoped to a
  waiting point or handed out more than once (§4.4). Deriving the token means nothing stored
  continues a run.
- **Claimed under the lock.** Two requests carrying one token at once resumed it twice before
  (CHANGELOG rc.25).
- **Sealed at rest.** A waiting run's record holds what its run was given; the deployments file is
  not the place for a secret in the clear (rc.25).
- **A waiting run holds nothing; it checks at commit.** Holding a row for days would block every
  other writer; checking at the commit and running again from the last wait keeps it correct
  (rc.27, `approved-payout`).
- **Pending is not absent.** Treating "not yet" as absent would send a fallback request, or park on
  an `await`, before learning it isn't needed ([lazy choices §4](../../L4s/lazy-choices-L4.md)).

## 4. Reach and limits

- **Defaults and ceilings** (`DurableLimits`): `max_await_seconds` 30 days by default, at most 90
  (`MAX_AWAIT_SECONDS`); `max_waiting_runs` 10,000 by default, at most 100,000. A deploy whose
  `await` waits longer than granted is refused, naming the limit to ask for.
- **A crash between a resumed part's commit and the deployments file's write runs it again.** The
  run is still parked after the restart; the same event resumes it again. Its database write
  applies once (the commit is marked done), but its outbound requests go out again: its record of
  the first resume was lost with the stop (`a_resumed_run_resumed_again_after_its_commit_applies_once`
  asserts two hits).
- **Each park and each resume writes the whole deployments file**, every deployment in it, not a
  record of its own; not measured.
- **Expiry is a second's sweep of every waiting point**, under the deployments lock to list them,
  on the clock scheduler's thread, after its ticks: a slow tick delays expiry. `expire_waiting`
  doesn't look at a deployment's status, so a paused deployment's runs expire on time, while
  its resume routes answer 503.
- **A run sealed under a key the server no longer has** can't be opened: it is said at start and
  stays parked, and its resume fails.
- **A resumed run reads the state from when it started**; what it must read as it is now, it reads
  after the `await` from what it shares (a row, a file), which the commit check covers.
- **The resume route's caller waits for the resumed part.** A resumed part that calls out slowly
  holds up the answer to whoever opened the link.
- **No revoking one token** short of finishing or expiring its run; a new host key revokes all.

## 5. What the tests hold

In `crates/holon/src/tests.rs`:

- `a_durable_run_waits_resumes_by_token_and_expires`: the run waits, its write held; the resume
  route answers and delivers, the write happens once under the same run id; a run started before a
  redeploy finishes on its version; one nobody resumes expires; waiting runs survive a restart.
- `a_waiting_runs_record_is_sealed_on_disk`: the deployments file doesn't hold the caller's
  credential; the record is open in memory, opens only for its own deployment, and resumes after a
  reload.
- `a_token_resumes_its_run_once_however_many_requests_carry_it`: one claims, the others find
  nothing; the deadline doesn't expire it under the resume; once let go, the token works again.
- `waiting_is_held_to_the_granted_limits`: an `await` longer than granted refuses the deploy,
  naming `max_await_seconds=…`; a lowered grant shortens a new wait's deadline; past
  `max_waiting_runs` the route answers 503.
- `a_parked_run_releases_its_claims`, `a_resumed_run_whose_row_changed_runs_again_from_its_last_wait`
  (SQLite, and Postgres and MySQL when at hand), `a_resumed_run_whose_file_changed_runs_again_from_its_last_wait`,
  `a_resumed_run_resumed_again_after_its_commit_applies_once`,
  `approved_payout_keeps_the_deposits_made_while_it_waited`,
  `a_waiting_run_sends_what_didnt_wait_as_it_parks`.
- `a_deployment_runs_pauses_stops_and_keeps_every_session`; in `lifecycle.rs`,
  `a_deployment_runs_pauses_resumes_and_stops_with_one_session`,
  `each_run_after_a_stop_is_a_new_session_from_the_chosen_state`.
- In `durable.rs`, `a_runs_tokens_are_derived_stable_and_its_own`, and
  `an_approval_gives_its_run_every_decision_taken`: a table of the decisions taken on an approval
  for two people, and the value its run is given, `decisions` and all.
- `an_approval_is_decided_by_different_people_never_by_a_token`: no token is handed out for an
  approval or resumes it; a viewer, a service, an agent and the same person twice are refused;
  two people approve and the run goes on with who they were; one decline with a reason ends it; one
  that runs out goes on expired, with who approved; each decision is in the log and the activity
  log. `only_a_person_decides_an_approval`.
- In `mcp.rs`, `an_agent_reads_who_approved_a_waiting_run_and_never_decides_it`; in
  `decidecmd.rs`, how a waiting run, a decision and a run picked by its id's start read.

End to end: `approval` deploys and answers in `sweep.e2e.ts` (what it sends on its channel as a
run parks is `a_waiting_run_sends_what_didnt_wait_as_it_parks`), and `approved-payout waits for two
people's approval holding nothing, and pays from the balance as it is then` (`system.e2e.ts`): an invited
editor and the admin approve it through the decide route, the same person twice is refused, an
agent is refused, and a second payout one person declines pays nothing.

**Gaps.** Nothing tests a lost host key or secrets key, the deployments file's cost with many
waiting runs, or a paused deployment's runs expiring.

## 6. History

Durable runs, derived tokens and the host key predate the CHANGELOG's first entry (rc.15).

| Release or commit | Change |
|---|---|
| rc.15 | A waiting point inside a `map` or `fold` function is answered, per item |
| rc.16 | Lazy choices: an `await` in an untaken branch doesn't park; `holon verify` answers a run's own requests, not only its earlier parts' |
| rc.23 | A waiting run's progress goes out as it parks, logged as `progress` |
| rc.25 | A token resumes its run once; a waiting run's record is sealed on disk, the file owner-only |
| rc.27 | A run that waited checks what it shares at commit and runs again from its last wait (`approved-payout`) |
| `6bc5c68` | rc.29: clocks, file handlers and actions beside routes, each parking as a route does, resumed by the flow's resume routes |
| `b1259e3`, `f06e7a1` | Strict by default; a handler that waits can't be `lazy` or `unrecorded` |
| `836e406` | `approved-payout` moves to the system end-to-end tests |
| G12 | `await approval(…)`: a waiting point people signed in decide, two-person and more, never a token |
| G12 | `approved-payout` waits at `approval("payout", { people: 2 })`, its approve link gone; a call on a path not taken is no longer made |
