# Holon: working conventions

## Every feature ships with a sample flow

Holon is shown by what you can build with it. When a feature lands (a primitive, a node template,
a host capability such as secrets, outbound requests or durable runs), it lands with a **sample
flow** that uses it, in `editor/src/samples.ts`:

- a short name and a one-line description of what it demonstrates;
- built from the live catalog (never a hand-copied node shape), so it can't drift from core;
- covered by `editor/src/samples.test.ts`, which builds every sample and derives its routes
  through the real core: a sample that stops building fails the tests;
- run live once (deployed, sent a request) before the commit, and the commit says so.

A change to an existing feature updates the samples that use it. The editor's flows page lists
the samples; opening one starts an unsaved copy.

## Commits and pushes

One commit per step, ending with the `Co-Authored-By` line; commit or push only when asked.
