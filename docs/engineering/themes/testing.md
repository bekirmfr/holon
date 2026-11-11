# Testing

What the tests hold Holon to, which test holds which property, and why each exists. A test here is
named as it is in the code; search for the name.

## 1. The suites

| Suite | Run with | In CI (`.github/workflows/ci.yml`, on every pull request; `e2e.yml` for end to end) | Size |
|---|---|---|---|
| Rust workspace | `cargo test --workspace`, then `cargo clippy --workspace --all-targets -- -D warnings` | job `native`, with Postgres 17, MySQL 8.4 and a MongoDB replica set beside it (`HOLON_TEST_POSTGRES`, `HOLON_TEST_MYSQL`, `HOLON_TEST_MONGODB`) | about 1,180 tests, 5 ignored (measurements) |
| A sample from the CLI | `cargo run -p holon -- new … --sample slugs`, then `check` | job `native` | 1 |
| wasm parity | `wasm-pack test --node crates/holon-wasm` | job `wasm-parity` | 23 under wasm, 15 native |
| Editor | `npx tsc --noEmit -p .`, `npx vitest run` (`npm test` in `editor/`) | job `editor`, against a freshly built wasm | 357 at `905e9fc`, every sample built in `samples.test.ts` |
| Embedded editor | `cargo test -p holon --features embed-editor editor` | job `editor` | |
| End to end | `npm run e2e` in `editor/` (Playwright, the real editor and server) | `e2e.yml`, jobs `e2e (1)` and `e2e (2)`: on main, nightly, and a pull request labelled `e2e` or a release's (`release-…`) | 90 tests, one of them every sample deployed from its text (`sweep.e2e.ts`, about a minute); it skips `atlas-notes` and `ledger-watch` without `HOLON_TEST_MONGODB` and `HOLON_TEST_POSTGRES` |
| VS Code extension | `xvfb-run -a npm run test:vscode` (in `editors/vscode`: `holon lsp` in a real VS Code) | job `vscode` | |
| Golden records | inside `cargo test`; `HOLON_GOLDEN=update` rewrites them | yes | `crates/holon-author/tests/golden/eval.txt` (379 cases), `crates/holon/tests/golden/server.txt` (342 runs) |

Every Rust test runs as a debug build, so the debug-only checks (the redaction shortcut's, below)
run in all of them.

Most server tests are in `crates/holon/src/tests.rs`, driving the server through its routes as a
client would. `dataplane.rs`, `listeners.rs` and `units.rs` have no tests of their own; they are
tested from there.

## 2. What holds what

### Nothing changes by accident

The golden records are the floor every optimisation stands on.

- `the_evaluator_gives_what_its_golden_record_holds` (`holon-author/src/golden.rs`): every node's
  outputs over every sample and the `crunch` flow. It was added (`7eb8e22`) before the evaluator
  was touched, and caught a planted change to `concat`.
- `the_server_records_what_its_golden_record_holds` (`tests.rs`): the answers and log entries of
  every sample that needs nothing outside the server, with the clock, random numbers and nonce
  fixed (`firing::fixed`).
- `the_encoding_is_pinned` (`value_hash.rs`): the exact bytes and hash of a value. A change to the
  encoding changes every state root and run id; it must come with an `ENCODING_VERSION` bump.
- wasm parity (in `holon-wasm`, each run natively and under wasm): `check_fraction_parity`
  pins the hash of fraction arithmetic over random, everyday and raw-bit operands (subnormals
  included), and `check_signature_parity` signs with the RFC's test vectors, so those two agree
  across the targets bit for bit. `check_hash_parity` and `check_eval_parity` check the bridge
  against the core within one target (a unit's hash as the bridge gives it and as the core
  does; a few evaluations, NaN by `is_nan`): no unit's hash is pinned as a literal. That the
  editor's hashes are the server's is shown end to end by the samples the editor's own tests
  deploy through its Deploy (`samples.e2e.ts`); every sample's unit, built natively by
  `holon deploy` from the same `flow_unit`, is deployed by `sweep.e2e.ts`.

### Fast paths answer as the plain ones

Each faster path is tested against the one it stands in for, on many inputs.

- `a_composite_run_down_its_plan_does_what_the_walk_does` (`eval.rs`): 3,000 random graphs, the same
  observer events in the same order and the same outputs.
- `a_pass_through_found_not_true_hands_the_run_to_the_walk`,
  `a_plan_out_of_order_hands_over_and_a_kind_that_isnt_its_bodys_computes_nothing` (R3: writing it
  found the pass-through step answering absent instead of handing over),
  `a_run_whose_scratch_is_taken_uses_one_of_its_own` (S4), `a_composite_with_a_choice_is_walked`.
- `has_cycle_is_exactly_where_topo_order_finds_no_order`,
  `the_flat_index_finds_what_a_map_of_the_edges_finds`: the evaluator's indexes against the
  eager evaluator's.
- `lazy_and_eager_agree_on_generated_graphs`, `every_samples_routes_give_what_the_eager_evaluator_gives`.
- `keeping_each_composites_shape_changes_nothing_the_evaluator_gives` (step 1b).
- `a_cut_back_keeps_its_lines_byte_for_byte_without_parsing_them`: the indexed cut-back against the
  old one that parsed every line.
- `a_line_made_before_the_lock_is_the_one_serializing_writes` (12.4c).
- `a_state_changed_exactly_when_its_snapshot_hash_moved`,
  `a_collection_read_is_asked_again_only_when_its_root_moved`,
  `a_store_failing_at_the_commit_check_fails_only_a_run_whose_collection_moved` (step 2's one
  change of behaviour, `1c562de`).
- `a_line_with_nothing_to_redact_is_the_redacted_line`, and inside `redacted_line` a debug-build
  check that the shortcut's line equals the redacted one on every logged run (D3);
  `with_nothing_to_mark_a_text_without_a_mark_sign_stays_as_it_is`: 5,000 awkward texts.
- The timer's size bound: `the_bound_holds_over_random_values_of_every_kind` (20,000 values, D4),
  `the_bound_is_never_below_the_json`, `outputs_are_kept_exactly_when_their_json_fits`.

### Tripwires: fail when a fast path is left

A slower path that is still right would pass every test above; these fail instead.

- `the_compute_heavy_flows_functions_all_run_down_a_plan` (D5): at least 5 of `crunch`'s functions
  are planned (`would_plan`). Fails with the pass-through guard removed.
- `every_genesis_primitive_is_computed_somewhere_and_every_opcode_is_one` (D1): every primitive is
  reached by an opcode or the host, every opcode is a primitive. A name left out used to mean a
  node that silently computes nothing.

### Durability

- `a_strict_run_is_on_disk_before_its_answer_and_a_lazy_one_waits_for_the_syncer`: strict reads,
  writes and actions sync before answering; lazy ones and 404s add no sync; 32 strict runs at once
  take at most one sync each.
- `one_sync_covers_every_entry_written_before_it`, `sync_logs_syncs_what_was_written_and_keeps_a_log_that_fails_dirty`,
  `a_log_is_dirty_from_a_write_until_it_is_synced_and_a_cut_back_syncs_it`.
- `a_run_writes_only_its_state_head_and_only_when_it_changed_the_state`,
  `a_state_is_kept_by_its_head_across_restarts_concurrent_writes_and_a_delete`.
- `corrupt_persistence_file_is_an_error_not_an_empty_table` (a corrupt file once loaded as empty,
  and the next save overwrote every deployment),
  `unloadable_records_are_an_error_and_state_always_survives_a_restart`,
  `persist_writes_whole_files_and_leaves_no_temp_files`.
- The chain: `each_entry_names_the_line_before_it_and_the_chain_survives_cutting_back`,
  `a_line_changed_removed_or_inserted_outside_holon_is_a_break_at_the_entry_after_it`,
  `a_kept_log_reads_back_after_a_restart_skips_a_cut_line_and_stays_bounded`.

### Isolation

- `a_light_route_answers_on_the_data_planes_worker_as_any_route_does` (12.1, R2): which runs answer
  on the worker and which on the pool, then over a real socket.
- `a_run_that_panics_is_answered_500_inline_as_on_the_pool` (R1).
- `one_deployments_log_doesnt_hold_up_another`: another deployment answers in under 400 ms while a
  busy log is written and cut back.
- `a_fire_in_progress_does_not_stall_the_table`, `effect_written_to_state_does_not_take_the_server_down`:
  both regressions (a persist held the table's lock waiting on a state lock; a panic under a
  state lock poisoned it for every later request).
- `concurrent_runs_lose_no_update`, `a_slow_run_holds_up_no_other_and_retries_without_resending`.
- `a_closure_too_big_to_run_is_refused_and_every_sample_is_well_inside`,
  `a_composite_nested_as_deep_as_a_host_runs_fits_half_a_threads_stack`.

### Log cut-backs and sweeps

- `a_cut_made_outside_the_lock_leaves_the_file_one_made_holding_it_leaves` (R4: byte for byte,
  entry after entry), `lines_written_while_a_cut_is_made_outside_the_lock_are_all_kept_in_order`
  (8 threads), `a_cut_overtaken_by_the_file_changing_is_made_afresh_and_a_deleted_log_stays_deleted`,
  `a_file_changed_outside_holon_is_read_afresh_by_its_cut_back`.
- `a_log_seals_its_older_runs_and_reads_them_back_as_it_wrote_them` (step 9: against a log in
  memory, run for run and line for line, the cut outside the lock and the one holding it alike,
  every run let go of backed up), `a_run_sealed_and_still_in_the_file_after_a_crash_is_read_once`,
  `the_states_sealed_runs_left_are_kept_by_a_sweep`.
- `a_log_on_file_keeps_its_newest_runs_in_memory_and_reads_the_rest_from_its_file` (R8): against
  a log keeping them all in memory, run for run; `a_read_planned_before_a_cut_back_reads_the_runs_it_planned`.
- `a_log_read_a_line_at_a_time_holds_no_lock_and_sees_one_file_whole` (R5): taking the lock inside
  the reader's callback would never return if it were held.
- `the_server_sweeps_its_store_on_its_own_and_not_again_for_nothing` (R6),
  `the_server_sweeps_as_often_as_its_storage_setting_says`,
  `an_admin_sets_how_often_the_server_sweeps_in_its_policy_file` (its other sections and comments
  kept; the e2e `rolling-log` test sets it from the admin page),
  `the_host_says_what_it_holds_in_memory_and_on_its_disks`,
  `each_flow_says_what_it_holds_in_memory_and_on_disk`,
  `the_host_warns_of_a_full_disk_and_a_full_cache_once_each`,
  `a_full_disk_pauses_what_records_runs_until_there_is_room`, `a_full_disk_rolls_the_log_keeping_the_most`,
  `the_values_a_sweep_keeps_are_packed_and_read_back` (and swept from the pack once named no
  more), holon-host's `packed_values_read_back_as_put_and_a_rewrite_leaves_the_rest`,
  `a_pack_cut_short_is_said_and_the_others_read` (and a count past anything overflows nothing),
  `a_pack_that_cant_be_read_is_warned_of_and_the_store_opens`,
  `a_rewrite_hides_no_value_it_keeps_and_keeps_one_put_again` (a thread reading throughout),
  `the_runs_a_log_lets_go_of_are_backed_up_as_exports_verify_checks` (with the log, every run once;
  files of the folder's own left alone and uncounted; a batch that can't be exported holds up no
  other; a backup that can't be written keeps them in the log),
  `a_backup_batch_and_its_export_are_known_by_their_names_and_nothing_else_is`.
- From the review ([Lessons](lessons.md#what-the-review-found)), each failing without its fix:
  `keeping_more_again_forgets_nothing_the_log_was_doing`,
  `a_backup_written_to_again_is_no_longer_an_error`, `a_cut_whose_swap_fails_leaves_no_segment_behind`,
  `a_full_disk_rolling_sweeps_what_only_the_cut_runs_named`,
  `a_full_disk_rolls_the_log_biggest_with_its_sealed_runs`; and in
  `a_full_disk_pauses_what_records_runs_until_there_is_room`, a `full_at` reached before `warn_at`
  pauses once and holds.
  `what_a_crash_left_is_removed_once_it_is_old_and_nothing_else_is`,
  `a_write_made_aside_is_known_by_its_name_and_nothing_else_is`,
  `a_store_sweep_clears_the_states_no_kept_run_names_and_nothing_else`,
  `a_journal_keeps_what_its_runs_read_in_chunks_for_a_while_and_stays_under_its_cap`,
  `a_hash_is_exactly_64_hex_digits_anywhere_in_a_text`.

### Unrecorded runs

- `an_unrecorded_run_leaves_no_entry_failed_or_not_and_is_counted`,
  `an_unrecorded_failure_is_kept_redacted_and_cut_short`,
  `a_policy_without_allow_unrecorded_refuses_the_deploy`, `a_timer_that_is_off_keeps_nothing`;
  in the editor, `tallies.test.ts`.

### Caches

- `a_closure_is_kept_by_its_hash_and_its_units_are_shared`, `a_partial_closure_is_loaded_again_once_it_is_whole`,
  `a_closure_keeps_its_composites_shapes_for_its_own_units`.
- `the_flow_cache_lets_go_of_the_least_used_past_its_budget`,
  `the_store_keeps_the_values_it_read_within_a_budget`,
  `the_value_caches_size_is_a_setting_it_keeps_to` (S5). They read the sizes they expect from the
  cache, so a change to the footprint estimates doesn't break them.

### Hostile values

What a request or a flow can hand a leaf, and how deep a flow can nest, are held on a thread a
host's size (2 MiB), since an overflowed stack aborts every deployment on the server:

- `every_pure_leaf_over_hostile_values_answers_finitely_and_the_same_twice`
  (`holon-core/tests/audit_leaves.rs`): every pure leaf, each family value a leaf of its own and the
  higher-order ones given ten functions (an inconsistent comparator among them), over a pool of
  edge numbers, awkward texts and nested values, a list and a record as deep as a value may be
  among them: about 480 variants, 222,000 evaluations, none panicking, answering a number that
  isn't finite or a value nested past the bound, answering differently twice, or taking 2 s. It
  prints the slowest; `HOLON_AUDIT_TRACE=1` names each evaluation, for when one overflows.
- `no_pure_leaf_panics_on_a_number_that_isnt_finite` (the same file): the same variants over NaN
  and the infinities, alone and inside a list and a record. No leaf answers such a number, but a
  module may (a `js` function's `1 / 0`), and the pool above holds none. It found a number written
  beside a text panicking (`number_text`), and `number_format` in a radix asking for 64 GiB, which
  aborts the server where a panic is answered 500.
- How deep a value nests (256, `MAX_VALUE_DEPTH`): `a_leaf_puts_nothing_past_the_depth_a_value_may_nest`
  (each leaf that puts a value down, at the bound and one short of it, lent and given),
  `no_leaf_builds_a_value_nested_past_the_bound` (every leaf, a value at the bound or one short of
  it in each input: one that nests what it is handed without saying so fails),
  `a_value_built_a_level_at_a_time_stops_at_the_bound` (a fold, a map, a group, a reviver), and
  `a_value_nested_a_level_a_step_stops_at_the_bound_and_the_server_answers_on` (a list and a record
  at the bound answered, recorded and read back from the log; one past it, or 10,000, a 422).
- Reading back JSON holding such a value: `json_holding_a_value_as_deep_as_one_may_be_reads_back`
  (and JSON 100,000 deep refused before anything recurses),
  `a_log_holding_a_value_as_deep_as_one_may_be_reopens_whole` (every run, no break), and
  `a_durable_run_waits_resumes_by_token_and_expires` (a waiting run holding one survives a restart).
- `the_deepest_route_a_server_runs_fits_a_runtimes_thread` (`tests.rs`): a route nested to
  `MAX_CLOSURE_DEPTH`, composites in composites and maps in maps, through the server's own
  resolver, in a debug build.
- The bounds where a few bytes make much more: `a_text_padded_or_repeated_stops_at_the_bound`,
  `a_laid_out_text_stops_at_the_bound`, `a_replacer_nesting_without_end_gives_nothing`, and
  `a_cell_never_holds_what_it_couldnt_load` (a value too deep to read back isn't written to state).
- `a_flow_call_sends_and_hands_back_data_only`: an effect is neither sent to another flow nor
  handed back by one.

### Secrets

Redaction is held by many tests in `secrets.rs` and `tests.rs`: a secret reaches its runs and
nothing else, a value computed from one (base64, escaped, a field of it) stays out of the log, a
look-alike of a placeholder is taken as text, a waiting run's record is sealed, a secret is never
sent to another flow. See the coming security doc.

### Samples

Every sample is a test ([`CLAUDE.md`](../../../CLAUDE.md)):

- `samples.rs`: each builds, with a unique name and port; its text says what its entry says; it is
  as `fmt` writes it; a mistake in one names it and the line.
- `editor/src/samples.test.ts`: each builds through the real wasm core against the live catalog;
  its names are unique, each is one of a server, a clock or a file handler, servers on ports of
  their own; its endpoints, clocks and file handlers are as many as its document's `request`,
  `every` and `on_file` templates; its capabilities ask for nothing unknown and no `Spawn`, and
  for the secrets and optional permissions its entry names, and its `flow(…)`'s `keep` and
  `limits` are the limits the deploy asks for.
- `editor/e2e/sweep.e2e.ts`: each deploys from its text with `holon deploy` to the real server,
  its port moved, and a request sent to it is found in its log; then it is removed.
- `editor/e2e/samples.e2e.ts`: the flows page lists every sample; a sample is opened and deployed
  through the editor, its consent screen included, only for what the editor alone shows.

## 3. Measurements, run by hand

None of these fail on a slower build; they print numbers. See [Measuring](measuring.md).

| Test | Run with | Measures |
|---|---|---|
| `request_throughput` | `cargo test --release -p holon -- --ignored request_throughput --nocapture` (`HOLON_PERF_SECS`) | requests a second, p50 and p99 at 1, 8 and 64 threads, beside 30 other deployments, in process |
| `profile_one_route` | `HOLON_PROFILE=hello\|notes\|crunch HOLON_PROFILE_N=2000 cargo test --release -p holon -- --ignored profile_one_route`, under callgrind | one route, repeated, for a profiler |
| `measure_cache_memory` | `cargo test --release -p holon -- --ignored measure_cache_memory --nocapture` (Linux, glibc) | each cache's bytes in use (`mallinfo2`) against what it counts |
| `measure_log_memory` | `HOLON_MEASURE_LOG=path/to/slug.jsonl cargo test --release -p holon -- --ignored measure_log_memory --nocapture` (Linux, glibc) | a log's entries' bytes in memory against the lines they were written as (1.1 to 3.6 times, on the soaks' logs) |
| `what_a_call_costs` | `cargo test -p holon-module --test js_engine -- --ignored` | a js call's cost, cold and warm |

## 4. Gaps

Known, and not yet covered:

1. Nothing shows an **unrecorded** route reading state, a collection or a secret going to the pool:
   every pooled case in the isolation test is also lazy, strict or calling out. `CollectionRead`
   put back in `READS_HERE` would likely pass.
2. The `Here` memo keeping only the live version's endpoints (S2) after a redeploy.
3. The caches against real memory: only by hand, and it asserts nothing; a return to 2.8–7.8 times
   their size would pass CI.
4. The counting rule (D2) has no tripwire of its own, only the unrecorded tests.
5. The directory sync after a cut-back's swap.
6. The soak's flat lines (memory, threads, files, latency): no automated check; the soak script is
   not in the repo.
7. Tallies written by the syncer when only they changed, across a real restart.
8. No end-to-end latency or isolation check: the cross-deployment figures come from `wrk` runs by
   hand.
9. The differential harness predates most of the library's leaves (its `LEAVES` list has the
   older ones the library changed, `slice`, `index_of`, `json_stringify`, but not `sort`, `flat`,
   `merge` or the date and math leaves) and runs no case through holon-host's sourced resolver;
   `differential.sh` builds only from `5f29d1d` on.
