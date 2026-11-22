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
