# holon-author

The language, its compiler and printer, the routes a flow derives, the samples, and what the
command line, the language server and the agent tools say of a flow. Written for someone who will
change it.

## 1. What it is

`crates/holon-author` turns a flow, written as `.holon` text or drawn on the editor's canvas, into
the units core runs, and back. It is pure like core (no I/O, its `Cargo.toml` says so), so it is
the one implementation everywhere: the editor runs it compiled to wasm through `holon-wasm`, and
`holon` runs it natively for `new`, `check`, `show`, `fmt`, `lsp`, `mcp`, `deploy` and the
terminal editor. Its dependencies are `holon-core`, `holon-module` (the js engine's bytes, which
lowering a `js` statement needs) and serde.

The language's reference is [`docs/LANGUAGE.md`](../../LANGUAGE.md); this doc is about the code.
In `src/`: `lang/` is the language (`lex.rs`, `ast.rs`, `parse.rs`, `lower.rs`, `print.rs`,
`layout.rs`); `text.rs` the first wiring-form parser and printer, and `restore`, which puts a
document's units in a store and which everything uses; `builder.rs`, `canvas.rs`, `templates.rs`,
`group.rs`, `composites.rs`, `modules.rs` and `wire.rs` build flow documents from the live
catalog and give the JSON forms the editor reads; `routes.rs` and `flow.rs` derive servers;
`commands.rs`, `editing.rs`, `agent.rs`, `describe.rs`, `usage.rs` and `graph.rs` say what the
command line, an editor, an agent and the terminal editor show. The samples are `samples/*.holon`
with their entries in `src/samples.rs`; the golden harness is `src/golden.rs` and
`tests/golden/eval.txt`; `tests/manual.rs` compiles every flow in `docs/manual`.

## 2. How it works

### Text to units

`lang::compile(src)` is `lower(&parse(src)?)`. `lex` makes tokens (durations, interpolated strings
with the columns of each `{…}` part, a `js` body read as JavaScript); `parse` builds a `File` of
headers and items, both forms of the text into one tree, every node with its `Span`; `lower` walks
the tree onto `FlowBuilder`, each expression becoming nodes from the live catalog and wires, and
answers a `Lowered`: the flow's `name`, its `doc` (a `FlowDocument`, the same one the canvas
saves), `names` (each top-level node's name in the text, by id) and `spans` (each node's span).
`lay_out` places the nodes on a canvas: a band per connected group, a column per depth. A mistake
is a `LowerError` with its span, its message, and, for a name the header doesn't declare,
`declare`: the header line that fixes it.

Lowering is sugar over leaves (LANGUAGE.md §5.4): `state.k` is one `store_read`, `notes[k] = v` one
`collection_put`, a `match` is `equals`, `guard`, `or`, `not` and a `coalesce` chain
([match L4 §4](../../L4s/match-L4.md)). A holon is a composite frozen with its declared boundary.

### Units to a server

`routes.rs` finds the handlers in a document's nodes and wires. `components` groups node ids by
wires in either direction (a union-find over ids only, so a replay's nodes group the same way).
`find_routes` makes each group holding one `request` template a `RouteGroup`, refusing two
requests or two triggers wired together, a method not in core's `METHODS`, a path `parse_path`
can't read, and two routes that would match the same requests; a group with no request is not
part of the server and is skipped silently. `find_clocks`, `find_files` and `find_actions` do the
same for `every`, `on_file` and `action`. `flow::derive_server` groups each through core like a
selection, narrows the boundary (`route_spec`, `action_spec`, `clock_spec`, `file_spec`: the
trigger's input in; the response and every effect out; an optional input such as an
`http_request`'s `headers`, by `is_optional_input`, left unbound), wraps it (`endpoint_unit`,
`clock_unit`, `file_unit`, `action_unit`, `with_log` for a `lazy` or `unrecorded` handler), and
puts the handlers in one `server_unit` with the port. `flow_unit` is the unit a flow deploys as:
its server when it has handlers, else the whole canvas frozen as one composite.

### Units to text: `holon fmt`

`lang::print` writes a document as canonical text (LANGUAGE.md §11): headers, holons inner before
outer, routes in the order `find_routes` finds them, nodes inline where they have one consumer,
names from the text where it gave one. What the expression form can't spell prints in the wiring
form. `print_lines` also says the line each top-level node is printed on.

`commands::fmt` is `fmt_declaring` without its list: `with_declarations` compiles the text,
adding the header line any `LowerError` carries in `declare` until it compiles; the result is
printed, and `with_comments` puts the source's comments back above the statement each was above,
found by what that statement says. A text with any other mistake is not formatted.

### The commands

`commands.rs` takes a file's name and text and answers a `Report` (`out`, `err`, `code`);
`crates/holon/src/flowcmd.rs` reads and writes the files. `new_flow` is `STARTER` or a sample's
text as it is. `check` says the routes, clocks, file handlers and actions, the unit's hash, what
the flow may do (core's `deployed_capabilities`, each said by `describe_item_sending` with where
each secret is sent), its module budgets and unrecorded handlers, or `STARTS_NOTHING`; a mistake
is `file:line:col: message`, the line and a caret, and each js body is compiled by the engine
without running (`js_mistakes`). `show` outlines each node by its text name, with what feeds it.

### The language server

`holon lsp` is `crates/holon/src/lsp.rs`, which frames JSON-RPC and converts positions to UTF-16;
what it says is `editing.rs`'s: `mistakes` (the first mistake; or, compiled, the flow's problems,
each at its node when it is one node's, else at the `flow` header), `hover`, `complete`, `format`
(`fmt`), and `declare_missing` as a quick fix. The web editor calls the same functions through the
bridge (`holon_mistakes`, `holon_hover`, `holon_complete`, `holon_format`), and two more:
`highlight` (`holon_highlight`) and `runnable_by_hand` (`holon_runnable_by_hand`, the quick fix
for a flow nothing starts). `editors/vscode` starts `holon lsp`, with a TextMate grammar for colours.

### The agent plan

`agent::plan(source)` is what deploying a text sends, computed without a server: it compiles,
derives the server (refusing on its errors), takes `flow_unit`, and answers a `Plan`: `name`,
`unit`, `units` (the closure as preimages, children first), `document` (with its `trace` and the
text's `bindings`), `routes`, `port`, `interval_seconds`, `requirements`, `browsers`, `may`
(consent names, with each module's budget as a `Limit(...)` item) and `sent` (core's
`sent_secrets`). `holon mcp`, `holon deploy` and `holon editor` deploy from it, and `holon flows
save` saves from it.
`text_of` reads a document back as text (its saved `source` when that still plans, with any
missing declarations added, else the canvas printed); `explain` and `run_view` show a recorded run
as the program with each statement's value; `reference` is the language reference an agent reads.

### Samples

Each sample is a `.holon` file in `samples/`, embedded by `include_str!` in an entry of `SAMPLES`
(`Sample`: `name`, `title`, `description`, `shows`, `port` or `interval`, `secrets`, `optional`,
`browsers`, `try_it`, `text`). `samples::build` compiles the text and carries it as the document's
`source`, so the text pane opens on it and a deploy keeps its names. The editor reads them through
the bridge (`samples`, `build_sample`); `holon new --sample` writes the text. There are 53, counted
as the files in `samples/` and the `include_str!` lines in `samples.rs`, one each.

[`CLAUDE.md`](../../../CLAUDE.md) makes a sample part of every feature: a `.holon` file in the
canonical form with its entry, built by `samples.test.ts`, run live once before the commit (the
commit says so, as `5877fa0` does), and deployed by the end-to-end tests.

### Golden records

`golden.rs`'s `record` compiles every sample and `CRUNCH` (a compute-heavy flow of `map`s and
`fold`s), derives each, and evaluates every endpoint and action with three bodies (none, a record
with the fields samples read, a list of 60 numbers) and every clock once, through
`eval_observed`, writing every node's outputs in order (a value past 160 characters cut, with its
hash). The result is compared with `tests/golden/eval.txt`, 355 cases (counted as its `== `
lines); the first line that differs is named with its case. `HOLON_GOLDEN=update cargo test -p
holon-author golden` writes it afresh.

## 3. Why it is this way

- **One implementation, pure.** Lexer, parser, lowering and printer live here and nowhere else;
  no TypeScript copy (LANGUAGE.md §2, P5). The editor, the command line, the language server and
  an agent read a flow alike, because they call the same functions
  ([language-server L4](../../L4s/language-server-L4.md), decision 1).
- **Text lowers onto the builder, against the live catalog.** The canvas and the text produce the
  same document, so `flow_unit` and `derive_server` apply unchanged and a text's hash is its
  graph's; names, comments, positions and statement order are in no unit (LANGUAGE.md §2 P3, §7).
  Each noun lowers to one leaf, so the manifest is still derived from leaves (P6); `match` is the
  same unit as its guards written by hand ([match L4 §4](../../L4s/match-L4.md)).
- **The wiring form stays.** A canvas can make port names the expression form can't spell; the
  printer falls back to `node`/wire statements there, and old files keep working (LANGUAGE.md
  §10).
- **Positions are not part of the program.** A `.holon` file outside the editor carries no
  layout and is laid out on import; the editor keeps positions in its saved document, matched by
  name. Coordinates in the text were refused: a program would carry its picture and every diff
  would show drags ([text-pane L4](../../L4s/text-pane-L4.md), decision 8).
- **One mistake at a time, as `check` says it.** The compiler stops at the first; recovering
  would make the words differ from `check`'s, and the second mistake is often the first's
  ([language-server L4](../../L4s/language-server-L4.md), decision 3).
- **fmt mends one mistake.** It prints from the compiled document, so a text must compile; the
  one fix-it is a header line for a name the body uses undeclared, so files written before
  declared names migrate with one `fmt`
  ([declared-names L4 §5](../../L4s/declared-names-L4.md)). It keeps comments, which it once
  dropped against §4 of the language ([text-pane L4](../../L4s/text-pane-L4.md), decision 6).
- **Samples are text, compiled.** A sample written as a hand-copied node shape would drift from
  core; compiled against the catalog, one that stops compiling fails the tests. Kept as `fmt`
  writes it, it is readable documentation of the feature.
- **The agent plan computes only.** It reads what core and the language compute; the server side
  moves it. The document an agent deploys is the one the editor would
  ([agent-surface L4](../../L4s/agent-surface-L4.md), decision 5).
- **Golden records before the evaluator was touched.** Recorded at `7eb8e22` so the
  interpreter's steps could be shown to change no answer
  ([interpreter-path L4 §4](../../L4s/interpreter-path-L4.md); [Lessons](../themes/lessons.md)).
- **A module by engine and params.** The js engine's preimage is 3.2 MB, so a document names the
  engine and params and the reader rebuilds the unit and checks its hash (`modules.rs`;
  [js-node L4 §3](../../L4s/js-node-L4.md)). The engine's bytes ride in this crate, so the
  bridge's bundle grew to 6 MB from 1.9 ([js-node L4 §7](../../L4s/js-node-L4.md)).

## 4. Reach and limits

- **One diagnostic, whole text.** `mistakes` answers the first compile error only, and every
  change compiles the whole text: no incremental parsing ("a flow is small", the language server
  note, decision 2).
- **The language server** has no go-to-definition or rename; completion offers words, not
  snippets; each file is a flow on its own. Its note says "no code actions yet"; `declare_missing`
  is one now. The quick fixes are split: `holon lsp` declares names, the web editor makes a flow
  runnable by hand, and neither offers the other's.
- **fmt** formats only a text that compiles, and mends only undeclared secrets and state keys.
  Comments are re-anchored by the statement they sat above; a canvas edit reprints from the canvas
  and drops them (the text pane says how many).
- **Two wiring-form parsers.** `text.rs` keeps the first one and its printer; only parity tests
  reach them now (`every_sample_in_the_wiring_form_parses_as_the_old_parser_reads_it`,
  `every_sample_in_the_wiring_form_lowers_to_the_same_unit_as_its_text`), and the first skips
  samples with a `js` statement, which the old parser predates.
- **Leaves called by name** (`store_read(key: …)`, a wiring-form `node … = secret {…}`) are not
  checked against the header ([declared-names L4 §10](../../L4s/declared-names-L4.md)).
- **A canvas group with no request** is left out of the server without a word from `find_routes`.
- **Pure means blind to the host.** `check` can't know whether a secret is set, a folder bound or
  a port free; the server says so at deploy.
- **The golden record** gives no inputs to file handlers (`server.files`), so the samples that run
  on new files are not in it; it holds values, not effects (the server's `tests/golden/server.txt`
  holds those). `HOLON_GOLDEN=update` rewrites the whole file, so its diff is the review.
- **The sample rule is a convention.** Nothing fails when a feature lands without a sample or a
  sample isn't run live; the commit message is the only record of the live run.

## 5. What the tests hold

- **Samples** (`samples.rs`): `every_sample_builds_with_a_unique_name_and_port`;
  `a_samples_text_says_what_its_entry_says` (name, port, interval, browsers, optional permissions
  and secrets agree between the text and the entry, both ways for secrets);
  `every_sample_is_as_fmt_writes_it` (`commands::fmt` leaves each file as it is; each opens with a
  comment); `every_samples_routes_give_what_the_eager_evaluator_gives` (every route, with a body
  and without, against core's eager evaluator, at least 40 cases).
- **Round trips** (`lang/`): `every_sample_round_trips_from_its_text` and
  `every_sample_round_trips_from_its_builder` (the printed text lowers to the same unit hash, and
  printing again changes nothing); `the_canonical_form_of_two_samples`;
  `a_match_is_the_same_unit_as_its_guards_written_by_hand`; a test per construct in `lex.rs`,
  `parse.rs`, `lower.rs` and `print.rs`, `errors_say_the_line_and_column_and_what_was_expected`
  among them.
- **Commands** (`commands.rs`): `fmt_is_stable_and_every_sample_round_trips_through_new`,
  `fmt_keeps_the_comments`, `fmt_declares_what_the_body_reads_and_says_so`,
  `check_says_where_each_secret_is_sent`, `check_compiles_each_js_body_and_says_a_mistake_at_its_line`.
- **Routes** (`routes.rs`): `one_route_per_group_with_a_request_node`,
  `every_group_that_cant_be_a_route_says_why`, `a_route_keeps_the_response_first_and_every_effect`.
- **Editors and agents**: `editing.rs`'s `every_sample_has_no_mistakes`, `format_is_fmt`,
  `the_quick_fix_declares_what_the_body_uses`; `lsp.rs` on framed messages; `agent.rs`'s
  `a_plan_carries_the_unit_its_units_its_trace_and_its_names`, `a_plan_says_each_mistake_as_check_does`.
- **Golden** (`golden.rs`): `the_evaluator_gives_what_its_golden_record_holds`;
  `keeping_each_composites_shape_changes_nothing_the_evaluator_gives`; the tripwire
  `the_compute_heavy_flows_functions_all_run_down_a_plan` (at least 5 of `CRUNCH`'s functions pass
  `would_plan`). `tests/manual.rs`: `every_flow_in_the_manual_compiles`.

Beyond the crate, `editor/src/samples.test.ts` builds every sample through the wasm core and
`editor/e2e/samples.e2e.ts` deploys every one ([editor](editor.md)); CI's `native` job runs
`holon new --sample slugs` and `holon check`. The crate had 146 tests at `5877fa0` (its commit
message). Gaps: no test ties the editor's deploy document to `agent::plan`'s; file handlers are not
in the golden record; no test sends a sample its `try_it`.

## 6. History

This clone's history begins at `6bc5c68` (2026-10-27); the language was built before it, as
LANGUAGE.md §12 records.

- `6bc5c68` release rc.29: triggers, actions and flows that call each other, in the language.
- `a598533` a `keep` line sets how long a log keeps runs.
- `64e78a9` `holon editor`, the terminal editor; `agent.rs` gains what it reads.
- `5877fa0` every sample says in its comments what it does and how to try it; the fmt test
  compares with `commands::fmt`, comments kept.
- `9ba3eb1` a clock's, file's or action's run placed by its label.
- `6720dca`, `9c5751d` the header declares the secrets and the state keys a flow reads.
- `60697af` the quick fix that declares a name used undeclared.
- `14c7163` the approval, and `check`, say where each secret is sent.
- `6f31e60`, `e3a6f80` `graph.rs` and `usage.rs`, for the terminal editor's graph and inspector.
- `c6a254e` `limit` lines set a deploy setting's limit from the text.
- `f06e7a1` `lazy` and `unrecorded` on a handler's line, checked against what it does.
- `7eb8e22` the golden records of what the evaluator and the server give.
- `440eca0` the record checked again with each composite's shape kept.
- `9969f8b` the tripwire on `CRUNCH`'s plans.
