# Request path: implementation plan

**Companion to** `request-path-L4.md`, the design. This file says how to build it. Steps are in the
design's build order (§6). Each step names the files, functions and lines it changes at `aa2704a`,
sketches the new code, lists the tests it keeps green and the ones it adds, and gives the checks to
run and the numbers its commit should report. Code here is a sketch of shape and names, not final
code. Where the design leaves a decision open (§8), the step that needs it says so.

## 0. Conventions for every step

- **One commit per step** (`CLAUDE.md`), each ending with the `Co-Authored-By` line. Its message gives
  the step's before and after numbers from the throughput check (step 1), and, for a step a person
  can see, that it was run live (deployed, then sent requests).
- **Checks.** These are the commands CI runs (`.github/workflows/ci.yml`):
  - every step: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`;
  - a step that touches `holon-core` or `holon-author` also runs
    `wasm-pack test --node crates/holon-wasm`;
  - a step that touches `editor/` runs, in `editor/`, `npm ci`, `npx tsc --noEmit -p .` and
    `npx vitest run`, then `npm run build` and `cargo test -p holon --features embed-editor editor`;
  - a step that touches samples or what the editor deploys runs `npx playwright test`
    (`editor/e2e`).
- **Lock order** (design §5), checked by review in every step that takes a lock:
  - never take the deployments table while holding a deployment's log lock or a state cell;
  - clone `Arc`s under the table and release it before taking a log lock;
  - the closure cache's lock is a leaf.
- **No migration.** Holon is in development, so no step carries existing flows or data folders
  forward. A flow whose hash changes is redeployed, and a data folder from before step 7 loads its
  stateful deployments empty. No step adds a reader for an old format, and step 7 removes the ones
  its change replaces.

| Step | Change | Crates | Changes on disk | Needs a decision |
|---|---|---|---|---|
| 1 | The throughput check | `holon` (tests) | none | |
| 2 | C1, closures by hash | `holon` | none | Q2, the bound (default 100,000 units) |
| 3 | C2 (a)(b), the log's own lock and the open file | `holon` | none | |
| 4 | C2 (c), cut-back without parsing | `holon` | none | |
| 5 | C2 (d), lazy sync and the witness hook | `holon` | none | Q7, the interval (default 1 s) |
| 6 | C3 (a), persist on change | `holon` | none | Q1, counters |
| 7 | C3 (b)–(e), the state head | `holon` | adds `deployments.state/`, drops `state_refs` (no migration) | |
| 8 | C5 in the language and core | `holon-core`, `holon-author`, `holon-wasm`, `editor` (types) | a new param in handler units, only for flows that use a word | |
| 9 | C2 (e) and C5 in the server | `holon`, `holon-core`, `holon-author`, `holon-wasm`, `editor`, docs, a sample | none new | Q9, Q10, Q11, Q12 |
| 10 | C3 (f), the log line as the state commit | `holon` | the head file goes | only if step 9's numbers ask |
| 11 | C4, the store lock | `holon`, `holon-host` | none | only after re-measuring |
| 12 | §4.5 (a), the per-run constant | `holon` | none | Q16, for its last item |

## Step 1. The throughput check

**Goal.** A repeatable before-and-after measurement that every later commit reports, with no external
tool.

**Where.** A new `#[test] #[ignore] fn request_throughput()` in `crates/holon/src/tests.rs`, beside
the data-plane helpers.

**How.**
- **State on disk**, so C1's and C2's costs are real. The test helper uses an in-memory store
  (`test_state`, `tests.rs:14–16`), which hides them. Build the state the way the on-disk test does
  (`tests.rs:918`): `test_state_with(SharedStore::on_disk(Box::new(FileStore::new(&dir)?), values))`,
  then set `state.deployments_path = unique_temp_path("throughput")`, so the logs and
  `deployments.json` are written, as `tests.rs:1281` does.
- **Deploy from the samples' text**, through the helper that approves what the preview asks for
  (`deploy_text_to`, `tests.rs:4832–4846`, which uses `holon_author::agent::plan`):
  `include_str!("../../holon-author/samples/hello-api.holon")`, `calc.holon` and `counter.holon`,
  plus 30 copies of `calc` renamed, `text.replace("flow calc", &format!("flow calc{i}"))` with the
  port changed likewise.
- **Drive** `serve_data(&state.store, &state.deployments, &state.deployments_path, slug, &fake_data(…))`
  (`fake_data`, `tests.rs:64`; as `data_call` does, `tests.rs:4851–4855`) from N threads in
  `std::thread::scope`, for `HOLON_PERF_SECS` (default 5) per route and per N of 1, 8 and 64.
  Each thread records each call's `Instant` duration. The routes are `GET /hello/Ada`, `POST /calc`
  with `{"op":"mul","a":6,"b":7}`, `GET /hit` and `GET /last`.
- **Print** one line per route and N: req/s, p50 and p99. Assert only that every answer is 200, so
  the test never fails on timing.

**Limits.** It calls `serve_data` directly, so it doesn't measure hyper or the hop onto the
blocking pool (`dataplane.rs:295`). Steps 9 and 12, which change those, also report a live
`wrk -t2 -c64` and `wrk -t1 -c1` run against `holon serve`, as the design's Appendix A describes.

**Run it with** `cargo test --release -p holon -- --ignored request_throughput --nocapture`.

**Commit:** `tests: a throughput check over the data plane (ignored; run by hand)`, with today's
numbers. Expect the design's §3 "none" row in shape: no gain from 8 to 64 threads, and `/last` and
`/hit` far below the others.

## Step 2. C1: closures by hash

**Code.**
- **The cache**, in `units.rs` beside `materialize_closure` (`units.rs:159–184`):
  ```rust
  pub(crate) const CLOSURE_CACHE_UNITS: usize = 100_000;            // §8 Q2
  #[derive(Default)]
  pub(crate) struct Closures(Mutex<Cache>);
  #[derive(Default)]
  struct Cache { map: HashMap<Hash, Kept>, units: usize, tick: u64 }
  struct Kept { closure: Arc<MapResolver>, units: usize, used: u64 }

  /// Walk and fetch as today; also say whether every reference resolved and how many units.
  fn load_closure(store: &SharedStore, root: Hash) -> Result<(MapResolver, bool, usize), Closure>;
  /// Today's API, kept for its tests (tests.rs:2945, 7197, 7216, 7223).
  pub(crate) fn materialize_closure(store: &SharedStore, root: Hash) -> Result<MapResolver, Closure> {
      load_closure(store, root).map(|(r, _, _)| r)
  }
  /// The closure of `root`, from memory when it was loaded whole before.
  pub(crate) fn closure(store: &SharedStore, root: Hash) -> Result<Arc<MapResolver>, Closure>;
  ```
- **What `closure` does.**
  - It locks the cache only to look up (`used = tick`) and to insert.
  - It calls `load_closure` with no cache lock held.
  - It inserts only when `whole`. A `store.get` that answers `Ok(None)` (`units.rs:168–170`) sets
    `whole = false`.
  - It evicts the least recently used entries while `units` exceeds the bound, and never caches a
    closure bigger than the whole bound.
  - Errors aren't cached.
- **Where the cache lives.** `SharedStore` (`main.rs:281`) becomes
  `SharedStore(Arc<Mutex<DynStore>>, Arc<calls::Hub>, Arc<sweep::Puts>, Arc<units::Closures>)`.
  `new` and `on_disk` (`main.rs:286–295`) add `Arc::default()`. The existing `.0`, `.1` and `.2`
  uses (`tests.rs:23`, `sweep.rs:165, 199`, `folders.rs:1683`) are unchanged.
- **Callers.** Each `materialize_closure(store, h)` becomes `closure(store, h)`. `resolver.resolve(&h)`
  works through `Deref`, and `WithEngine::new(&resolver, …)` becomes `WithEngine::new(&*resolver, …)`:
  - per run: `listeners.rs:95` (with `WithEngine` at `listeners.rs:134`), `scheduler.rs:125`,
    `arrivals.rs:373`, `deploy.rs:924`, `deploy.rs:1079`, `durable.rs:637` (`WithEngine` at
    `durable.rs:640`);
  - the rest: `deploy.rs:162`, `deploy.rs:507`, `deploy.rs:749`, `persist.rs:132` (which warms the
    cache at start), `grants.rs:19`, `firing.rs:761` and `units.rs:256`.
- **Thread safety.** `MapResolver` (`holon-core/src/resolve.rs:34–37`) is a `BTreeMap<Hash, Unit>`, so it
  is `Send + Sync`. Nothing inserts into a resolver after it is built.

**Tests.** New, in `tests.rs`:
- `a_closure_is_kept_by_its_hash`: two calls give `Arc::ptr_eq`;
- `a_partial_closure_is_loaded_again`: put a flow whose child is missing, call `closure`, put the
  child, call again; the second resolves the child;
- `a_failed_closure_is_not_kept`;
- `the_closure_cache_forgets_the_least_used_past_its_bound`: with the bound lowered through a
  `#[cfg(test)]` constant.

Unchanged and must pass: the closure-bound tests (`tests.rs:7190–7225`) and every redeploy and
rollback test, now through the cache.

**Measure.** The throughput check. Expect `hello` and `calc` to rise at 64 threads and fall back at
1. In the design's §3, `cache` alone took `hello` from 1,575 to 3,820 req/s.

**Commit:** `units: closures kept in memory by their hash (C1)`.

## Step 3. C2 (a)(b): each deployment's log behind its own lock, written through an open file

**Code.**
- **The cell**, in `recorder.rs`:
  ```rust
  pub struct LogCell { log: Mutex<Log>, next_seq: AtomicU64 }
  pub struct Pushed { pub seq: u64, pub end: u64 }        // `end`: the byte offset after its line (step 9)
  impl LogCell {
      pub fn new(log: Log) -> LogCell;
      pub fn next_seq(&self) -> u64;                        // the atomic, no lock
      pub fn with<T>(&self, f: impl FnOnce(&mut Log) -> T) -> T;
      pub fn push(&self, entry: LogEntry) -> Pushed;        // lock, push, mirror next_seq
  }
  ```
- **`Log` keeps its file open.** `Log` (`recorder.rs:354–375`) gains `handle: Option<File>`.
  `keep_line` (`recorder.rs:521–549`) opens it once, with the `create_dir_all`, `OpenOptions` and
  `mode(0o600)` it does today, then each line is one `write_all`. `cut_back` (`recorder.rs:554–593`)
  sets `handle = None` before `write_private`, so the next line reopens. `delete_file`
  (`recorder.rs:625–629`) drops the handle first. `push` (`recorder.rs:497–516`) returns the line's
  end offset.
- **`Deployment.log` becomes `Arc<LogCell>`** (`main.rs:483`).
  - A new deployment wraps `Log::for_deployment` at `deploy.rs:339` and at load, `persist.rs:221–222`.
  - A redeploy (`deploy.rs:338`) clones the old `Arc` instead of `mem::take`, then
    `log.with(|l| l.retain(…))` (`deploy.rs:341`).
- **`log_run`** (`firing.rs:786–799`) returns `Option<Pushed>`:
  ```rust
  pub(crate) fn log_run(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, mut entry: LogEntry, tokens: Redactor) -> Option<Pushed> {
      let (log, secrets, channels, permit, session) = {
          let table = deployments.lock().unwrap();
          let dep = table.get(slug)?;
          (Arc::clone(&dep.log), Arc::clone(&dep.secrets), Arc::clone(&dep.channels), Arc::clone(&dep.permit),
           dep.life.sessions.last().map_or(0, |s| s.number))
      };                                                      // table released
      let answered = std::mem::take(&mut entry.answered);
      tokens.and(Redactor::of(&secrets)).and(Redactor::answers(&answered.0, &secrets)).entry(&mut entry);
      entry.session = session;
      log.with(|l| {                                          // this deployment only
          let packs = channels.packs_of(&entry, l.next_seq());
          let pushed = l.push(entry);
          for (name, pack) in packs { channels.send(&name, pack, &permit.channels); }
          pushed
      })                                                      // and mirror next_seq into the atomic
  }
  ```
  Packs are still computed and sent under the log lock, in the same order as the entry
  (`firing.rs:790–797`: "what a subscriber gets is what the log holds").
- **Readers** take the `Arc` under the table, release the table, then call `log.with(…)`. The sites
  are `deploy.rs:841`, `deploy.rs:1155–1165` (where `after`, `next_seq`, `head` and `breaks` come
  under one `with`, so they agree), `deploy.rs:1419`, `deploy.rs:1451`, `deploy.rs:1526`,
  `changes.rs:64`, `collections.rs:148`, `verify.rs:100` (lines and head under one `with`),
  `sweep.rs:137` and `folders.rs:1702` (clone every `Arc` under the table, then read each),
  `grants.rs:47`, and the 3 direct uses in `tests.rs`.
- **Readers that only need `next_seq`** read the atomic: `events.rs:205` (`Deployment::note`, called
  under the table, so it must not wait on a log lock), `events.rs:226` and `deploy.rs:1161`.
- **B.1 fixed.** `run_action` uses the `seq` `log_run` returns (`deploy.rs:1118–1119`) instead of
  reading `next_seq` first.

**Tests.** Unchanged and must pass: `recorder.rs` 710, 733, 765, 798, 835 and 877, and `tests.rs`
3785, 3813, 4174, 7419, plus the redaction tests 4308, 4398, 4570, 4722 and 6079 (design C2). New:
- `an_actions_reported_entry_is_its_own`: 8 threads run one action at once, and each answer's `seq`
  names an entry whose `run_id` matches the run;
- `one_deployments_log_doesnt_hold_up_another`: a test holds deployment A's `log.with` on one
  thread while deployment B answers on another, with a timeout.

**Measure.** The throughput check at 64 threads. Steps 2 and 3 together are where the server starts
to scale. Expect `hello` well above step 2. The design's upper bound is about 21,000 req/s, with no
log at all.

**Commit:** `recorder: each deployment's log behind its own lock, through an open file (C2 a, b)`.

## Step 4. C2 (c): cut-back without parsing

**Code.**
- **An index of lines.** `Log` gains `lines: VecDeque<LineAt>`, with
  `struct LineAt { offset: u64, len: u32, at_ms: u64 }`, one per line that parsed.
- **Filled at open.** `open_retaining` (`recorder.rs:409–448`) iterates `text.split_inclusive('\n')`
  instead of `text.lines()`, keeping a running byte offset. It pushes a `LineAt` for each line that
  parses, and skips a cut line exactly as today.
- **Kept on append.** `keep_line` pushes one after each write. The leading `\n` it writes after a
  cut (`recorder.rs:533`) moves the offset by one.
- **The cut itself.** `cut_back` reads the file's bytes (`std::fs::read`, with no `from_str`). It
  picks the kept lines from `lines` by today's rule (`recorder.rs:565–572`: the newest `keep`, none
  older than the age, always the newest), and appends a new `line` the same way. It joins the
  chosen byte ranges, writes them with `write_private`, and rebuilds `lines` with the new offsets.
  The witness's `trim` (`recorder.rs:583–588`) is unchanged.

**Tests.** The six `recorder.rs` tests pass unchanged. New:
- `a_cut_back_keeps_its_lines_byte_for_byte`: a file holding a cut line and a line with an unknown
  field is cut back. Its bytes equal what today's `cut_back` writes, checked against a copy of
  today's function kept in the test module.
- `appends_after_a_cut_back_land_in_the_new_file`.
- `offsets_survive_a_restart`: open, cut, reopen, append, cut again.

**Measure.** Time `cut_back` in a test over a 1,000-line, 5 KB-per-line file. Today it takes 31–36 ms
on the measuring machine; expect a few ms. The throughput check's p99 at 64 threads should drop.

**Commit:** `recorder: a cut-back copies the lines it keeps instead of parsing them (C2 c)`.

## Step 5. C2 (d): logs synced lazily, and before every witness send

**Code.**
- **Dirty flags.** `Log` and `Events` each gain `dirty: bool`. `Log::keep_line`
  (`recorder.rs:521–549`) and `Events::keep_line` (`events.rs:150–170`) set it.
  ```rust
  /// recorder.rs: each dirty log of the table, fsynced; a failure puts the flag back.
  pub(crate) fn sync_logs(deployments: &Mutex<BTreeMap<String, Deployment>>) -> Result<(), String>;
  ```
- **What `sync_logs` does.**
  - Under the table, it clones each `Arc<LogCell>`, and for each deployment whose `events` is dirty
    takes the flag and the file's path (events are pushed under the table, `events.rs:203–206`).
  - It releases the table. For each log it calls `log.with(|l| take(dirty), path)`.
  - With no lock held, it opens each taken path and calls `sync_data`.
  - A failure puts the flag back and returns the error.
- **The syncer thread.** `main.rs`, beside the threads at `main.rs:1075–1080`, spawns a loop of
  `sleep(SYNC_EVERY)` (1 s, §8 Q7) and `sync_logs`, printing a failure once until it clears.
- **The witness hook.** `heads.rs` gains
  `pub fn before_send(hook: Box<dyn Fn() -> Result<(), String> + Send + Sync>)`, stored in a
  `OnceLock` beside `EXPORTER` (`heads.rs:60`). `Exporter::flush` (`heads.rs:130–160`) calls it after
  taking a batch from `pending` (`heads.rs:134`) and before the POST (`heads.rs:141`). On `Err` it
  returns `Err` and sends nothing, and the back-off in `Exporter::run` (`heads.rs:181–200`) retries.
  `main.rs` installs the hook after the table exists (`main.rs:1074`), because `heads::start` runs
  before it (`main.rs:990`).

**Tests.**
- `recorder.rs`: an append sets the flag and `sync_logs` clears it; a log whose file was removed
  keeps its flag and returns the error.
- `heads.rs`: the existing tests that start a local witness (`heads.rs:234`, `heads.rs:286`) gain a
  hook that counts calls, and check it ran before each POST. A failing hook sends nothing, and the
  heads stay queued (`waiting()` unchanged).
- `tests.rs`: a line appended during a cut-back is synced on the next `sync_logs`.

**Docs.**
- `docs/manual/reference/server.md:163–176` gains "a run's entry is on disk within a second; a power
  loss can lose the last second of entries". Step 9 rewrites this sentence for strict.
- The witness paragraph (`server.md:180–182`) gains "a head is sent only once its entry is on disk".

**Measure.** The throughput check should match step 4's. Say so in the commit, since this is a
durability fix.

**Commit:** `recorder: logs synced lazily, and before every witness send (C2 d)`.

## Step 6. C3 (a): persist only when the run changed something

**Code.**
- **Deciding "changed".** `FiredRun` (`firing.rs:67–104`) gains
  ```rust
  /// It committed a state that differs from the one it started from.
  pub(crate) fn changed_state(&self) -> bool {
      match (&self.start_state, &self.state_after) {
          (Some(start), Some(after)) => crate::restore::snapshot_hash(start).ok().as_deref() != Some(after.as_str()),
          _ => false,
      }
  }
  ```
  `snapshot_hash` is `restore.rs:36`. It hashes in memory, using each cell's cached hash (`State::hash_of`).
- **`serve_flow`** captures `changed` in the `data::handle` closure, beside `answer_derived`
  (`listeners.rs:128`; the closure is `listeners.rs:139–160`). The condition at `listeners.rs:227` becomes `if changed || parked_any`.
- **The other run paths** change the same way: `scheduler.rs:196`, `arrivals.rs:411`, `deploy.rs:957`
  and `deploy.rs:1120`. `durable.rs:684` stays unconditional, because the set of waiting runs
  changed.

**Tests.** Unchanged and must pass: `trigger_with_state_persists_across_calls` (2242),
`clock_tick_with_state_persists_across_ticks` (3169), `state_persists_across_a_simulated_restart`
(3285) and `a_collection_keeps_items_by_key_across_runs_and_restarts` (2474). New:
- `a_run_that_only_reads_state_writes_nothing`: `deployments.json` is byte for byte the same after a
  `GET /last`;
- `a_write_of_the_same_value_writes_nothing`;
- the counters (§8 Q1): a test that pins whichever decision is taken.

**Measure.** The throughput check's `/last`. In the design's §3 `nopersist` row it went from 53 to
879 req/s, at 33 deployments.

**Commit:** `persist: a run persists only what it changed (C3 a)`.

## Step 7. C3 (b)–(e): a state head per deployment

**Code.**
- **Where the head lives.** `recorder.rs` (or `persist.rs`) gains
  `pub fn state_head_for(deployments_path: &str, slug: &str) -> Option<PathBuf>`, which is
  `Path::new(path).with_extension("state").join(slug)`, matching `file_for` (`recorder.rs:384`).
  It returns none for `:memory:`.
- **The cell writes it.** `StateCell` (`claims.rs:26–35`) gains `head: Option<PathBuf>` and
  ```rust
  /// Make `snapshot` (hex) this deployment's durable state: write_private, which syncs and renames.
  pub(crate) fn write_head(&self, snapshot: &str) -> std::io::Result<()>;
  ```
  Cells are made at `deploy.rs:382` and `persist.rs:182–186`; both pass the path.
- **When a run writes it.** In `fire_with_state`, right after `snapshot_after`, still holding `live`
  (`firing.rs:230`, `firing.rs:298`): `if let Some(s) = &a.run.state_after { cell.write_head(s) }`.
- **A failure.** Today a failed persist only warns (`persist.rs:87–89`). The plan keeps that parity
  (`eprintln`, plus `entry.error` saying the state wasn't kept), and asks in review whether a run
  whose head can't be written should answer 500 instead.
- **State replaced outside a run.** `put_state_back` (`deploy.rs:1613–1625`) and the lifecycle's run
  from a session (`deploy.rs:1707–1710`) call `save_snapshot(&live, None, store)` and then `write_head`,
  under the same lock.
- **A new stateful deployment** writes the empty state's head.
- **Load** (`persist.rs:172–196`): read the hex from `state_head_for(path, slug)`, then
  `restore::snapshot_refs(hex, store)` (`restore.rs:46–57`), then `State::load`. A stateful
  deployment with no head starts empty. A head naming a missing snapshot is an `Err`, as a bad
  `state_refs` is today (`persist.rs:180`). The old readers go: `state_refs` and the inline values before it, for the live
  state and for sessions (`persist.rs:172–196`, `migrate_inline`).
- **Delete** (`deploy.rs:841–845`) removes the head beside the log.
- **`persist_deployments`** (`persist.rs:22–92`) writes definitions only. The state loop
  (`persist.rs:65–81`) and `DeploymentRecord.state_refs` (`wire.rs:224–226`) go, and so does the
  record's inline `state` field (`wire.rs:227–230`).

**Tests.**
- `state_is_kept_by_reference_and_its_root_survives_a_restart` (`tests.rs:2059`) changes at
  `tests.rs:2081–2084` and `2103`: it reads the head's snapshot, not `state_refs` in the file.
- `inline_state_from_older_files_moves_into_the_store` (`tests.rs:2111`) is removed, with the reader
  it covers.
- New:
  - `a_writes_state_survives_a_restart_through_its_head`;
  - `a_stateful_deployment_without_a_head_starts_empty`;
  - `a_restore_and_a_run_from_a_session_write_the_head`;
  - `deleting_a_deployment_removes_its_head`;
  - `concurrent_writes_leave_the_head_naming_the_last_commit`: 16 threads `POST /visits/:id` on
    `counter`, then the head equals `snapshot_hash` of the live state;
  - `persist_writes_whole_files_and_leaves_no_temp_files` (`tests.rs:2173`) extended to
    `deployments.state/`.

**Docs.** The data-folder table (`docs/manual/reference/server.md:22–33`) gains
`deployments.state/`: "each stateful deployment's current state, as a snapshot hash".

**Measure.** The throughput check's `/hit`. Expect a gain like `nopersist`'s, with the head's fsync
on top, and no change when filler deployments are added.

**Commit:** `persist: each stateful deployment's state kept as a head of its own (C3 b-e)`.

## Step 8. C5 in the language and core

**Behaviour.** No run behaves differently yet. This step makes the words parse, lower, hash, check
and print, and makes core read them.

**Core** (`holon-core/src/declare.rs`):
- **The level and its reader:**
  ```rust
  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
  pub enum LogLevel { #[default] Strict, Lazy, Unrecorded }
  /// A handler unit's level: its frozen `log` param, absent for strict.
  pub fn log_level(unit: &Unit) -> Option<LogLevel>;        // None: a value that isn't a level
  /// Whether `handler` may be lazy or unrecorded: every capability sourced, none Await (C5's rule).
  /// Err names the first capability that isn't.
  pub fn may_record_lightly(handler: &Unit, resolver: &impl Resolver) -> Result<(), Capability>;
  ```
  `may_record_lightly` uses `capabilities` (`kind.rs:595`, failing closed) and `is_sourced`
  (`kind.rs:206–212`).
- **The structs.** `Route`, `Clock`, `FileHandler` and `Action` (`declare.rs:41–83`) gain
  `pub log: LogLevel`. `read_server` (`declare.rs:191–270`) fills it from each child, where it reads
  `resume` (`declare.rs:253`) and in the `every`, `on_file` and `action` branches
  (`declare.rs:208–231`).
- **Errors.** `DeclError` (`declare.rs:159`) gains `BadLogLevel { child: u32, value: String }` and
  `LogLevelNotAllowed { child: u32, label: String, capability: String }`. `read_server` checks the
  rule too, so a server unit built outside the language can't skip it.

**Language** (`holon-author/src/lang`):
- `ast.rs`: `RouteDef` (`ast.rs:210`), `EveryDef`, `OnFileDef` and `ActionDef` gain
  `log: Option<Name>`.
- `parse.rs`: one helper,
  ```rust
  /// After a handler's head: an optional `lazy` or `unrecorded`, before its block.
  fn log_word(&mut self) -> R<Option<Name>>;   // is_word("lazy") || is_word("unrecorded")
  ```
  It's called in `route` after `resumes` (`parse.rs:582–587`), and in `on_file`, `action` and
  `every` (`parse.rs:616–650`). The words stay contextual (`is_word`), so they remain names
  everywhere else.
- `lower.rs`: push `("log", word)` onto the trigger node's params, only when present, in `route`
  (`lower.rs:1056–1060`, beside `resume`), `every` (`lower.rs:1108–1111`), `on_file`
  (`lower.rs:1134–1142`) and `action` (`lower.rs:1171–1173`).
- `print.rs`: add `lazy` and `unrecorded` to the contextual words (`print.rs:31`), and print the word
  after `resumes` (`print.rs:732`) and after the other heads. `strict` is never printed.

**Units** (`holon-author/src/routes.rs`, `templates.rs`, `flow.rs`):
- `templates.rs:174–178`: `request`, `every`, `on_file` and `action` gain `f("log", "")`, which is
  what a fresh canvas node starts with.
- `routes.rs`:
  - `RouteGroup` (`routes.rs:17–27`) gains `log: String`, read like `resume` (`routes.rs:268`);
    `ClockGroup`, `FileGroup` and `ActionGroup` (`routes.rs:63–100`) gain the same, read from their
    trigger node.
  - `endpoint_unit` (`routes.rs:433`), `clock_unit` (`routes.rs:386`), `file_unit` (`routes.rs:402`)
    and `action_unit` (`routes.rs:458`) take `log: &str`, and push `frozen("log", log)` only when
    it isn't empty. The default then stays out of the unit, so a flow without a word lowers as it
    does today. That's tidy, not required, since nothing is migrated.
- `flow.rs`:
  - **The check.** `derive_server` (`flow.rs:139–260`) checks each handler whose level isn't strict
    with `may_record_lightly(&unit, store)`. On `Err(cap)` it finds the node: the first node of the
    handler's group (`node_ids`) whose own unit (`unit_for`, `flow.rs:28`) has a capability that
    isn't sourced or is an `Await`.
  - **The error.** It pushes the message to `errors`, as "GET /health can't be unrecorded: it writes
    the collection "notes"", worded through `describe_item` (`describe.rs:8`). It also pushes
    `Placed { node, message }` to a new field.
  - **The new field.** `DerivedServer` (`flow.rs:120–130`) gains `placed: Vec<Placed>`, and the
    drafts (`EndpointDraft`, `ClockDraft`, `FileDraft`, `ActionDraft`, `flow.rs:85–117`) gain
    `log: String`.
- **Where the error shows.**
  - `commands::check` (`commands.rs:174–233`): a placed problem is printed `file:line:col: message`
    with a caret, from `c.spans` (`Lowered.spans`, `lower.rs:60–65`), the way `mistake` does
    (`commands.rs:61–69`).
  - `editing::mistakes` (`editing.rs:40–48`) puts a placed problem at its node's span instead of the
    `flow` header.
- `holon-wasm`: `derive_server` (`lib.rs:703–706`) needs no change, since the new fields serialize.
- `editor/src/canvasOps.ts:55–63`: `DerivedServer` gains `placed` and each draft `log`.

**Tests.**
- `holon-core`:
  - `read_server` reads each level and refuses `log = "verbose"`;
  - `may_record_lightly` is `Ok` for `hello-api`'s route and `Err(StateWrite)` for `counter`'s
    `/hit`;
  - it is `Err` for a unit whose child doesn't resolve (saturated, the fail-closed case).
- `holon-author`:
  - each word parses, lowers to the param (like `lower.rs:2810`'s `resume` test) and prints back;
  - `fmt` round trip;
  - a flow without words has no `log` param on any handler unit;
  - `route POST /notes (req) unrecorded { notes[x] = y … }` fails `check` at the `notes[x] = y`
    line and column;
  - the word on each kind of handler.
- `editor`: `holonText.test.ts` (`deriveServer`, `holonText.test.ts:45`) for `placed`.

**Docs.** `docs/LANGUAGE.md` §5.1 (the `handler` grammar, `LANGUAGE.md:849–862`) and §5.11, and
`docs/manual/language/reference.md`: the words, their rule and the compile error. The runtime effect
is described in step 9.

**Commit:** `language: lazy and unrecorded on a handler's line, checked against what it does (C5,
core and language)`.

## Step 9. C2 (e) and C5 in the server

**Decisions first:** §8 Q9 (`allow_unrecorded`'s default), Q10 (failures of unrecorded handlers),
Q11 (the README headline) and Q12 (requests no route took). The plan assumes these defaults:
`allow_unrecorded = false`, failures recorded, the headline as Q11 drafts it, and unrouted requests
`lazy`.

**Strict sync, C2 (e)** (`recorder.rs`):
- **Group commit:**
  ```rust
  impl LogCell {
      /// Return once every byte before `end` is on disk; one sync_data covers every line before it.
      pub fn sync_through(&self, end: u64) -> std::io::Result<()>;
  }
  ```
  `LogCell` gains `synced: Mutex<u64>` and a sync handle (a second `File` opened on the log's path,
  so syncing needs no log lock). `sync_through` locks `synced`, returns if `synced >= end`, else reads
  the current end under the log lock, calls `sync_data` with that lock released, and stores the
  end. `cut_back` (step 4) sets `synced` to the new file's length and reopens the sync handle, under
  the log lock.
- **Each run's level.**
  - A request: `holon_core::log_level(endpoint)`, inside the `data::handle` closure
    (`listeners.rs:139`, where `endpoint` is in hand).
  - A clock: its handler unit (`scheduler.rs`). A file arrival: its handler (`arrivals.rs`). An
    action: `action.handler` (`deploy.rs:1079`).
  - A resumed run: the level of the unit it started on (`durable.rs:637`).
  - `/trigger` (`deploy.rs:924`) is a whole flow, not a handler, so it's strict.
  - A request no route took (`ran` is `None`, `listeners.rs:208`) is lazy.
- **The effective level** is the declared one, unless `Unrecorded(label)` isn't granted (below),
  which makes it strict.
- **After `log_run`, a strict run calls `sync_through(pushed.end)`** before it answers or returns:
  `listeners.rs:226` (before the `answer` return at `listeners.rs:237`), `scheduler.rs:160`,
  `arrivals.rs:407`, `deploy.rs:955`, `deploy.rs:1119` and `durable.rs:651`.
- **A failed sync** is a 500 for a request, with `entry.error` already written, since its record
  isn't known to be durable. Elsewhere it's printed.

**Unrecorded runs** (`listeners.rs` and the other run paths):
- **When to log.** An unrecorded run calls `log_run` only when it failed: `entry.error.is_some()`, or
  `answer.status >= 500`, or a budget failure. A failure is logged lazily.
- **Tallies.** `Deployment` (`main.rs:433–500`) gains `tallies: BTreeMap<String, Tally>`, with
  `Tally { runs, failed, total_ms, last_at }`. `record_trigger` (`deploy.rs:867–875`) gains
  `label: Option<&str>` and `ms: f64`, and updates the tally when the level is unrecorded. Its six
  callers pass them. The tallies persist with the record (`wire.rs:220` `DeploymentRecord`, and
  `persist.rs:33–61`), written when a definition persists (C3 c).
- **The log API.** `handle_get_log` (`deploy.rs:1155–1165`) adds `"tallies"`.

**Permissions** (`holon/src/permissions.rs`, `holon-core/src/manifest.rs`):
- **The requirement.** `Requirements` (`permissions.rs:274–295`) gains
  `#[serde(default, skip_serializing_if = "BTreeSet::is_empty")] unrecorded: BTreeSet<String>`, and
  ```rust
  /// These requirements with each handler of `root` declared unrecorded, by its label.
  pub(crate) fn with_unrecorded(self, root: &Unit, resolver: &impl Resolver) -> Requirements;
  ```
  It sits beside `with_module_limits` (`permissions.rs:301–307`), and is called wherever that is:
  `deploy.rs:223`, `deploy.rs:513` and `deploy.rs:754`.
- **The item.** `manifest_items` (`manifest.rs:28–51`) takes `unrecorded: &BTreeSet<String>` and adds
  `Unrecorded("GET /health")` for each. Its callers are `permissions.rs:331`, `holon-wasm/src/lib.rs:282`
  (which receives `asked.unrecorded` from the editor) and `manifest.rs:112`.
  `editor/src/holon.ts:296` sends it, from the drafts' `log` (step 8).
- **The policy.** `Policy` gains `allow_unrecorded` (`permissions.rs:394–414`, the file's `grants`
  section at `permissions.rs:437` and `permissions.rs:484`). `refuses_item` (`permissions.rs:509–529`)
  refuses `Unrecorded(…)` without it: "this host never lets a handler go unrecorded (holon.toml:
  grants.allow_unrecorded)".
- **The permit.** `Permit` (`permissions.rs:759–826`) gains `unrecorded: BTreeSet<String>`: the
  granted `Unrecorded` labels, while the policy allows them, built as `plain_http` is
  (`permissions.rs:809–814`). It also gains `pub(crate) fn may_skip_record(&self, label: &str) -> bool`.
  A revoke takes effect at the next run, through the existing grant changes (`grants.rs`).
- **The wording.** `describe_item` (`holon-author/src/describe.rs:8`) for `Unrecorded("…")`: "don't
  record runs of GET /health (its failures are recorded)".

**The editor, the terminal editor and the agent tools:**
- `editor/src/server.ts:709`: `getLog` returns `tallies`.
- The live log (`useViewMode.tsx`, `ViewPanes.tsx`) shows one row per unrecorded handler, and
  `DeploymentCard.tsx` shows its counts.
- `Manifest.tsx` shows the item through `describe_item`.
- `crates/holon/src/workbench`: the terminal editor's runs list shows the same rows.
- `crates/holon/src/mcp.rs`: the tool that reads a run answers "runs of GET /health aren't
  recorded" for an unrecorded handler.

**The sample** (`CLAUDE.md`):
- `crates/holon-author/samples/status-board.holon`: the design's §7 text, already checked to compile
  without the words.
- Its entry in `samples.rs` (`Sample`, `samples.rs:28–48`):
  - name `status-board`, title "Status board";
  - description "a log level per handler: strict by default, lazy, unrecorded";
  - shows `["log levels", "unrecorded", "lazy", "collections"]`, port `"8170"`;
  - its "try it" lines.
- `editor/src/samples.test.ts` builds it.
- The e2e deploy (`editor/e2e/samples.e2e.ts:55` approves what's asked) needs the e2e server's
  policy to allow unrecorded: `allow_unrecorded = true` in the `holon.toml` that `editor/e2e/server.mjs`
  writes, if Q9 keeps the default off.
- Run it live before the commit.

**Docs.**
- `docs/manual/reference/server.md:161`: "Every run is recorded, on disk before its answer, unless
  its handler is `lazy` or an approved `unrecorded`".
- The run-log section (`server.md:163–176`) gets the three levels.
- `docs/manual/reference/permissions.md:44–52` gets the `Unrecorded` exception and
  `allow_unrecorded`.
- `README.md:3, 19`: the headline (Q11).
- `CHANGELOG.md`.

**Tests.**
- Strict:
  - `a_strict_runs_answer_comes_after_its_sync`: a test hook on `LogCell` counts syncs, and the sync
    covering a request's offset happens before `serve_data` returns;
  - `concurrent_strict_runs_share_syncs`: 32 threads cause fewer than 32 syncs.
- Unrecorded:
  - `an_unrecorded_run_leaves_no_entry_and_counts`;
  - `its_failure_is_recorded`;
  - `revoking_unrecorded_records_the_next_run`;
  - `a_policy_without_allow_unrecorded_refuses_the_deploy`;
  - `an_agents_deploy_asking_for_it_stops_for_approval` (as the agent tests do, `agent.e2e.ts`);
- `a_request_no_route_took_is_lazy`;
- `a_lazy_runs_entry_is_on_disk_after_the_syncers_tick`.
- In `holon-core` and the wasm bridge: `manifest_items` with `unrecorded`.

**Measure.**
- The throughput check, and a live `wrk` run of the sample's three routes at 1 and 64 connections.
  Expect `/health` near the design's unrecorded figure (about 21,000–27,000 req/s here), `/notes`
  near lazy (estimated at 15,000–16,000), and `POST /notes` at strict's pace.
- Read-only routes with no word slow to strict's pace (about 6,000 here). Say so, with both numbers.

**Commit:** `server: runs recorded strictly by default; lazy and approved unrecorded handlers (C2 e,
C5)`, which says it was run live.

## Step 10. C3 (f): the log line as the state commit (only if step 9's numbers ask)

**When.** Do this when step 9's `/hit` shows writing runs bound by the second fsync (the head's)
rather than the log's.

**Outline.**
- **The ticket.** `StateCell` gains `next_ticket: AtomicU64`. A run takes a ticket under `live`, at
  commit (`firing.rs:203–237`).
- **The order.** `LogCell` gains `turn: (Mutex<u64>, Condvar)`. `log_run` for a writing run waits for
  its ticket's turn before `push`, then advances the turn.
- **Load** takes the state from the newest line on disk with a `state_after` (`Log::open_retaining`
  already reads them), and the head file goes.
- **A design note first**, covering the ordering argument (design C3 f), what happens to a run that
  fails between its ticket and its line, and dropping the head files (no migration: a data folder
  with heads loads empty).

## Step 11. C4: the store lock (only after re-measuring)

**When.** After step 9, with several stateful deployments written at once, and after reading
`sweep.rs` in full (`named`, `Puts`, `FORGET_AT`).

**Outline.**
- **The backend.** `SharedStore`'s backend becomes `enum Backend { Disk(FileStore),
  Locked(Mutex<DynStore>) }` (`main.rs:272–310`).
- **The sweep gate.** A new `Arc<RwLock<()>>`, taken shared by every put and get, and exclusively
  by the sweep's check and remove (`sweep.rs:198–212`, today `store.0.lock()`). That keeps the
  guarantee `Puts::put`'s doc states (`sweep.rs:64–66`).
- **`FileStore`** (`holon-host/src/lib.rs:614–690`) gains `&self` variants of `put` and `put_value`.
- **`folders::kept_text`** (`folders.rs:1683`) goes through `SharedStore::get_value`.
- **A new test:** a put racing a sweep never loses a value a committed run names.

## Step 12. §4.5 (a): the per-run constant

One commit per item. The throughput check doesn't see items 12.1 and 12.3, so report live `wrk`
runs.

### 12.1 Read-only handlers without the thread hop (−30 to −40 µs)

- **A per-flow flag.** C1's cache entry gains a per-flow `Arc<FlowFacts>`, computed once per hash:
  `{ light_routes: Vec<bool> }`, true where `may_record_lightly` holds (step 8). It's built on the
  first `closure` of a server unit.
- **`answer`** (`dataplane.rs:263–330`) looks up the deployment's unit hash and the route's flag
  before `spawn_blocking` (`dataplane.rs:295`), using `match_route`. The lookup is a brief table lock
  that `allowed_origin` takes anyway (`dataplane.rs:291`).
- **A light route** runs
  `tokio::task::block_in_place(|| serve_data_forwarding(...))`. The runtime is multi-thread
  (`dataplane.rs:68–76`), which `block_in_place` requires. A light route calls out to nothing, so the
  `forwarded` channel and `select!` (`dataplane.rs:293–302`) are skipped.
- **A strict light route** still waits for its sync inside `block_in_place`. That costs a worker for
  about 0.6 ms, as `spawn_blocking` costs a pool thread today.
- **Tests:** a light route answers the same as before; a route that calls out still goes through
  `spawn_blocking` (one `tests.rs` case per kind).

### 12.2 No timeline for unrecorded runs (about −20 µs)

- `recorder::Timer` (`recorder.rs:293–350`) gains `enabled: bool`, with `Timer::off()`. `enter`,
  `produced` and `leave` return at once when it's off.
- `RunWaits` gains `record: bool`. `run_attempt` (`firing.rs:560`) makes `Timer::off()` when it's
  false. `serve_flow` and the other run paths set it from the effective level (step 9).
- An unrecorded run that fails is then logged without a timeline. Alternatively, it is run again
  with the timer on to log it; that is cheap, because evaluation is pure and the run failed anyway.
  Decide in review.

### 12.3 The origin checked once (part of −5 to −15 µs)

- `answer` computes `allowed_origin` (`dataplane.rs:291`) and passes it into
  `serve_data_forwarding` (`listeners.rs:34–42`), which computes it again today (`listeners.rs:42`).
  The control-plane path (`handle_post_request`, `listeners.rs:311`) keeps computing its own.

### 12.4 Per-flow caches of run setup (rest of −5 to −15 µs)

Profile first: the design's §4.5 measured run setup at about 24 µs. Candidates, each cached in
`FlowFacts` by hash:
- the server's route table (`read_server` result, now re-read from the deployment);
- `deployed_capabilities` facts that `fire_with_state` recomputes;
- the parts of `run_id_of` (`firing.rs:711`) that depend only on the deployment.

### 12.5 Per-node outputs in recorded runs (§8 Q16)

Only after Q16 is decided. Dropping them changes what the editor's replay reads
(`editor/src/replay.ts`, `timeline.ts`), so it would be a step of its own with its own design note.

## Appendix. What each step leaves for the next

| After step | The server… |
|---|---|
| 1 | is unchanged, and measured |
| 2 | reads no flow from disk per run |
| 3 | does no disk I/O under the table lock on the run path (M0 restored); one deployment's log no longer holds up another |
| 4 | no longer stalls a deployment for 33 ms every 500 runs |
| 5 | syncs every log within a second, and never tells a witness a head the disk lacks |
| 6 | writes nothing for a stateful run that changed nothing |
| 7 | writes one small file for a run that changed state, whatever the number of deployments |
| 8 | understands `lazy` and `unrecorded` in text, canvas and `check`, without acting on them |
| 9 | records every run strictly by default, with the two opt-outs; read-only routes without a word slow to strict's pace |
| 10, 11 | only if measured: one fsync per writing run; no store-wide lock |
| 12 | runs read-only handlers without the thread hop, and skips what unrecorded runs don't need |
