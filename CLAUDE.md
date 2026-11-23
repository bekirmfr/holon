# Holon: working conventions

## Every feature ships with a sample flow

Holon is shown by what you can build with it. When a feature lands (a primitive, a node template,
a host capability such as secrets, outbound requests or durable runs), it lands with a **sample
flow** that uses it: a `.holon.ts` file in `crates/holon-author/samples/`, written in the language
(`docs/LANGUAGE.md`) in the canonical form `holon fmt` writes, with its entry (name, title,
description, what it shows, how to try it) in `crates/holon-author/src/samples.rs`; the editor and
`holon new --sample` read them through the wasm bridge:

- a short name and a one-line description of what it demonstrates;
- compiled from the text against the live catalog (never a hand-copied node shape), so it can't
  drift from core, and kept as `fmt` writes it (a test says so);
- covered by `editor/src/samples.test.ts`, which builds every sample and derives its routes
  through the real core: a sample that stops compiling fails the tests;
- run live once (deployed, sent a request) before the commit, and the commit says so;
- deployed by the end-to-end tests (`editor/e2e`, `npm run e2e`) against the real server, from
  its text through the command line (`e2e/sweep.e2e.ts`, which builds the unit the editor's Deploy
  builds) and sent a request, so a sample that stops deploying or answering fails them. What it
  does is held below the browser (the golden record, a Rust test of its own); a sample gets an
  end-to-end test of its own only for what the editor alone shows. CI runs them on main, nightly,
  and a pull request labelled `e2e` or a release's (`.github/workflows/e2e.yml`); before merging
  any other pull request whose change they cover, run them locally or label it.

A change to an existing feature updates the samples that use it. The editor's flows page lists
the samples; opening one starts an unsaved copy.

## The two translators stay on par

A flow is text and canvas at once, translated by hand each way: text into steps
(`crates/holon-author/src/lang/ts.rs` and `lower.rs`), steps into text (`lang/print.rs`). Keep
them on par. A change to how text becomes steps comes with the matching change to how steps
become text, in the same commit. A new leaf or language form is done only when it goes both
ways, or is on a listed exception with its reason in plain words. The tests that hold this:
`lang/agree.rs` (every text `lang::compile` takes is written back as the same steps),
`lang/on_par.rs` (every catalog leaf and template drawn alone goes both ways, `NOT_YET` the
exceptions; `OWED` the texts refused today), and the grid's `ROUND_TRIP_DEBTS`
(`crates/holon/src/tests/semantics.rs`). Their lists only shrink.

## Heavy tests

A few Rust tests hold nearly all the test time. They are marked `#[ignore = "heavy: …"]`, so
`cargo test --workspace` skips them, and `.github/workflows/heavy.yml` runs them, beside the other
checks, in two groups:

- the grids, which hold that a flow's text is translated to steps and run as it says:
  `tests::semantics::a_handler_does_what_its_text_says`, its clock's, queue's, file's, mail's,
  feed's and websocket's forms, and
  `tests::list_grid::a_list_methods_function_makes_what_it_says_item_by_item`;
- the records sweep, which holds how run records are kept, exported, chained and verified:
  `tests::no_line_changed_and_chained_again_passes_as_checked`.

They run on a pull request or a push to main whose change touches a file the group guards (listed
in `.github/file-rules.txt`, applied by `.github/what-changed.sh`), on a pull request labelled
`heavy` (both groups), every night (both), and by hand. A session that changes the translator
(`crates/holon-author`), the core crates or the server's run engine runs the grids by hand before
calling a step done:

    cargo test -p holon --bin holon -- --ignored --exact tests::semantics::a_handler_does_what_its_text_says tests::semantics::a_clocks_handler_does_what_its_text_says tests::semantics::a_queues_handler_does_what_its_text_says tests::semantics::a_files_handler_does_what_its_text_says tests::semantics::a_mails_handler_does_what_its_text_says tests::semantics::a_feeds_handler_does_what_its_text_says tests::semantics::a_websockets_message_does_what_its_text_says tests::list_grid::a_list_methods_function_makes_what_it_says_item_by_item

The grids come in two sizes, set by `HOLON_GRIDS`. The full set (`full`, and what runs when it is
unset) sends every handler all eight requests. The small set (`small`) sends the route handlers
two of them, and only checks the clock, queue, file, mail, feed and websocket handlers (each is
still compiled and written back, but not run); the list grid is the same at both sizes. A pull request or a push to main runs
the small set; the nightly run, a run by hand and a pull request labelled `heavy` run the full set.
Every full run also checks that the two requests alone show every kind of finding it shows (each
debt, and a flow's own rule under each condition), and fails naming the kind and where it showed
if they don't: then add a request that shows it to `SMALL_REQUESTS` in `tests/semantics.rs`. Only
the full set checks that most runs act as JavaScript (the two requests are the ones that differ
most); the small set prints that share without checking it. Put
`HOLON_GRIDS=small` before the command above for the quick check.

and the records sweep the same way when it changes how runs are recorded, exported or verified
(`… --ignored --exact tests::no_line_changed_and_chained_again_passes_as_checked`).

A new heavy test lives in `crates/holon/src`, at the top of its file. Mark it
`#[ignore = "heavy: about N minutes, …"]`, name it in the workflow's `GRIDS` or `RECORDS`, and add
the files it guards to that group in `file-rules.txt`. `tests::heavy` fails if a marked test isn't
named in the workflow, or the workflow names one that isn't marked, or a rule names a path that
isn't there.

The same file rules let a pull request skip work its change can't affect. Every job of
`ci.yml` still starts and reports, so required checks never wait:

- A pull request that only adds or edits documents (`CLAUDE.md`, `CHANGELOG.md`, `docs/`) runs
  nothing and every job ends green. Not documents a test or a build reads: the manual
  (`docs/manual/`), the engineering pages, `docs/LANGUAGE.md`, `README.md` and the design page a
  test reads count as code, and so does any document deleted or renamed (a test checks that every
  page the code and the manual cite is there).
- A pull request that only touches the editor (`editor/`, `editors/vscode/`, and documents) runs
  no Rust tests but one, the check of the names the docs cite (it reads the editor's files too);
  the editor, wasm and VS Code jobs run as always. The editor fixtures a Rust test writes
  (`editor/src/*.fixture.json`) count as Rust.
- A push to main and a run by hand run everything. A path the rules don't name counts as code.

When a Rust test starts reading a file in `docs/` or `editor/`, add it to `[inert-not]` or
`[editor-not]` in `file-rules.txt`; `tests::heavy` fails if a path a Rust source names there is
left out.

## Commits and pushes

One commit per step, ending with the `Co-Authored-By` line; commit or push only when asked.

## Talking to the owner

Write to the owner as one person to another, in plain words. This holds for every session's
answers, questions, summaries and reports to the owner.

- Say it in this order: where things stand, the problem, why it happens, then the ways to deal
  with it and which one you suggest.
- Use everyday words. Leave out internal names (a finding's number, a function, a branch, a kind
  of mark) unless the owner needs one to act; the first time you use one, say what it is.
- Give one concrete example of the problem: what someone does, and what they then see.
- Say who is affected and who is not.
- Keep sentences short. Ask one thing at a time, and say what each answer would lead to.

Notes between sessions, commit messages and code comments may stay technical.
