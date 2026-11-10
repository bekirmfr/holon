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
- deployed by the end-to-end tests (`editor/e2e`, `npm run e2e`) through the real editor and
  server, so a sample that stops deploying fails them. CI runs them on main, nightly, and a pull
  request labelled `e2e` or a release's (`.github/workflows/e2e.yml`); before merging any other
  pull request whose change they cover, run them locally or label it.

A change to an existing feature updates the samples that use it. The editor's flows page lists
the samples; opening one starts an unsaved copy.

## Commits and pushes

One commit per step, ending with the `Co-Authored-By` line; commit or push only when asked.
