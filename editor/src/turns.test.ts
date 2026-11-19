// The test worker's event loop turns between two tests, and a test given the runner's 60 s or
// more fails unless it lets the loop turn while it runs (vitest.setup.ts, run before every file).
//
// Why: a file of plain tests held the worker's thread from its first test to its last, and the
// runner's answers to the worker's reports sat unread past their 60 s limit: the whole run ended
// with exit 1 and every test passing (vitest.setup.ts says more).
//
// If it fails: vite.config.ts no longer runs vitest.setup.ts, or vitest went another way between
// tests; the suite can again exit 1 with nothing failing, naming no test.

import { expect, it } from "vitest";

let turned = false;

it("a plain test, which leaves the worker's loop something to do", () => {
  setImmediate(() => {
    turned = true;
  });
});

it("the next test starts once the loop has turned", () => {
  expect(turned).toBe(true);
});

// The runner's limit, as vitest.setup.ts counts it.
const RUNNER = 60_000;

it.fails("a plain test given the runner's 60 s fails, the loop never turning while it runs", { timeout: RUNNER }, () => {});

it("a test given the runner's 60 s passes when it lets the loop go round", { timeout: RUNNER }, async () => {
  await new Promise((r) => setImmediate(r));
});

// A timer already due when the loop comes round runs before the loop reads what came in, and the
// test goes on from there: the loop hasn't gone round. A zero-delay timer is due or not by chance
// (the row above waited on one and failed a whole run once); this one is made due by holding the
// thread past it before the wait.
it.fails("a test given the runner's 60 s whose only wait is a timer already due fails", { timeout: RUNNER }, async () => {
  const due = new Promise((r) => setTimeout(r, 0));
  const until = Date.now() + 5;
  while (Date.now() < until);
  await due;
});

it("a plain test given less passes: its own limit holds it under the runner's", { timeout: RUNNER - 1 }, () => {});
