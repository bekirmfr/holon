// Run before every test file (vite.config.ts, `test.setupFiles`): the test worker's event loop
// turns after every test, and a test given the runner's 60 s or more must let it turn while it
// runs. Held by turns.test.ts.
//
// Why: vitest goes from one test to the next on promises alone, never through the event loop, so
// a file of plain (synchronous) tests held the worker's thread from its first test to its last.
// The worker reports each test to the runner and waits for the runner's answer, 60 s at most
// (birpc's own limit, which vitest doesn't let a config change). The answer came at once and sat
// unread; when the loop turned at last, after the file, the limit's timer ran first. So a file of
// plain tests longer than 60 s ended the whole run with exit 1, every test passing, and vitest's
// `[vitest-worker]: Timeout calling "onTaskUpdate"`: documentText.test.ts, compiling every sample
// several times over, took 63 to 172 s in each of the 7 whole runs out of 18 that ended so, and
// 44 to 59 s in each of the 11 that didn't. It grows with every sample.
//
// If it fails: the suite exits 1 now and then with nothing failing, on a busy machine first, and
// the error names no test.

import { afterEach, beforeEach } from "vitest";

/** The machine's own setImmediate, taken before any test puts timers on a fake clock. */
const turn = globalThis.setImmediate;

/** How long the worker waits for the runner's answer to a report (birpc's DEFAULT_TIMEOUT). */
const RUNNER_ANSWERS_WITHIN_MS = 60_000;

let turned = true;
beforeEach(() => {
  turned = false;
  turn(() => {
    turned = true;
  });
});

afterEach(({ task }) => {
  // A plain test is held to its own limit, so one under the runner's never holds the thread past
  // it. One given as much or more must let the loop go round while it runs, past where it reads
  // what came in (the runner's answers among it) to where setImmediate's callbacks run, or be
  // split. Most waits do. A timer already due when the loop comes round runs before that read, and
  // so does a read already done, and the test goes on from there: a test whose only wait is one of
  // those has not let the loop go round, and is refused too. A zero-delay timer is due or not by
  // chance, so `setImmediate` is the wait that always does.
  if (task.timeout >= RUNNER_ANSWERS_WITHIN_MS && !turned)
    throw new Error(
      `"${task.name}" is given ${task.timeout} ms but never lets the worker's event loop go round: the runner's answers to the worker wait ${RUNNER_ANSWERS_WITHIN_MS} ms at most. Split it, or let the loop go round between its parts (await new Promise((r) => setImmediate(r))).`,
    );
  return new Promise<void>((done) => turn(() => done()));
});
