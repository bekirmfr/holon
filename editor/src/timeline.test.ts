import { describe, expect, it } from "vitest";
import { buildTimeline, filterOf, itemText, parsePoint, restoreReport, restoreParts, restoredLine, runOptions, stateLines, targetOf, versionOptions } from "./timeline";
import type { DeploymentEvent, LogEntry, VersionEntry } from "./server";

const run = (seq: number, at_ms: number, extra: Partial<LogEntry> = {}): LogEntry => ({
  seq,
  session: 1,
  at_ms,
  duration_ms: 1,
  kind: "request",
  route: "GET /quote",
  method: "GET",
  url: "/quote",
  status: 200,
  request: null,
  sourced: [],
  state_before: {},
  outputs: [],
  performed: [],
  response_body: "",
  error: null,
  ...extra,
});
const event = (seq: number, run_seq: number, kind: string, what: string, extra: Partial<DeploymentEvent> = {}): DeploymentEvent => ({
  seq,
  at_ms: 0,
  run_seq,
  kind,
  what,
  ...extra,
});
const version = (number: number, extra: Partial<VersionEntry> = {}): VersionEntry => ({
  number,
  unit: `u${number}`,
  deployed_at: 0,
  current: false,
  restorable: true,
  canvas: true,
  state_kept: true,
  ...extra,
});

describe("the timeline", () => {
  it("puts what was done before the run it names, newest first", () => {
    const items = buildTimeline(
      [run(0, 10), run(1, 20)],
      [event(0, 0, "deploy", "deployed version 1", { version: 1 }), event(1, 1, "deploy", "deployed version 2", { version: 2 }), event(2, 2, "restore", "put back the state run 0 left, in session 2")],
      [version(2, { current: true }), version(1)],
    );
    expect(items.map((i) => i.key)).toEqual(["e2", "r1", "e1", "r0", "e0"]);
    const deploy = items.find((i) => i.key === "e1");
    expect(deploy?.kind === "event" && deploy.version?.number).toBe(2);
  });

  it("places a version deployed before events at its first run, or by time", () => {
    const items = buildTimeline([run(0, 1_000), run(1, 5_000)], [], [version(2, { deployed_at: 3, current: true }), version(1, { first_seq: 0 })]);
    expect(items.map((i) => i.key)).toEqual(["r1", "v2", "r0", "v1"]);
  });

  it("files each kind under its filter", () => {
    const kinds = buildTimeline([run(0, 0)], [event(0, 0, "grant", "approved Clock"), event(1, 0, "lifecycle", "paused"), event(2, 0, "restore", "x")], [version(1)]).map(filterOf);
    expect(kinds).toEqual(["runs", "versions", "lifecycle", "access", "versions"]);
  });

  it("words each row as it shows, for the filter to match", () => {
    const texts = buildTimeline([run(3, 0)], [event(0, 3, "grant", "approved Clock", { by: "maria" })], [version(2, { restored_from: 1 })]).map(itemText);
    expect(texts).toEqual(["run #3 GET /quote s1", "grant approved Clock by maria", "deploy restored version 1's code, as version 2 by the host"]);
  });
});

describe("restore points", () => {
  it("says what a version can't give back", () => {
    expect(versionOptions(version(3, { current: true }))).toEqual({ code: "it is the version deployed now", state: "the state it left is the live one" });
    expect(versionOptions(version(2))).toEqual({ code: null, state: null });
    expect(versionOptions(version(1, { restorable: false, state_kept: false })).code).toMatch(/rc\.29/);
  });

  it("gives a run's code only when it isn't deployed now", () => {
    expect(runOptions(run(4, 0, { unit: "u2", state_after: "ab" }), "u2")).toEqual({ code: "it ran on the code deployed now", state: null });
    expect(runOptions(run(4, 0, { unit: "u1" }), "u2").code).toBeNull();
    expect(runOptions(run(4, 0, { unit: "u1" }), "u2").state).toMatch(/no state/);
  });
});

describe("a restore's words", () => {
  const change = { from: "version 1", changed: ["#collection:prices"], added: [], removed: ["note"], runs_since: 3, effects_since: { HttpRequest: 2 } };

  it("says the keys and what the runs since did outside", () => {
    expect(stateLines(change)).toEqual([
      "The state version 1 left is put back: it changes #collection:prices; drops note.",
      "3 run(s) logged since version 1 also did, outside its state, what a restore doesn't undo: 2 call(s) out.",
      "The state it replaces is kept, as the ending session's.",
    ]);
    expect(stateLines({ ...change, changed: [], removed: [], runs_since: 0, effects_since: {} })[0]).toMatch(/changes no key/);
  });

  it("asks for approval when the code needs it", () => {
    const asked = restoreReport("prices", { label: "version 1" }, [{ point: { version: 1 }, what: { code: true, state: true }, preview: { code: 1, needs_approval: ["Clock"], missing_secrets: [], state: change } }], (i: string) => `read ${i}`);
    expect(asked.action).toBe("Approve and restore");
    expect(asked.title).toBe('Restore "prices" to version 1: its code and state?');
    expect(asked.sections.map((x) => x.heading)).toEqual(["code", "state"]);
    expect(asked.approvals).toEqual(["read Clock"]);
    const plain = restoreReport("prices", { label: "run 7" }, [{ point: { run: 7 }, what: { code: false, state: true }, preview: { code: null, state: change } }]);
    expect(plain.action).toBe("Restore");
    expect(plain.sections.map((x) => x.heading)).toEqual(["state"]);
    expect(plain.approvals).toEqual([]);
  });

  it("says what it did", () => {
    expect(restoredLine([{ point: { version: 1 }, done: { code: 1, version: 4, state: { from: "version 1", session: 3 } } }])).toBe(
      "restored: version 1's code is deployed again, as version 4; the state version 1 left is live again, in session 3 (session 2 keeps the one it replaced)",
    );
  });
});

const change0 = { from: "", changed: ["k"], added: [], removed: [], runs_since: 1, effects_since: {} };

describe("going back to a row", () => {
  // Newest first: a pause after run 2; a restore of run 0's state; version 2 deployed after run 1.
  const timeline = buildTimeline(
    [run(0, 0, { unit: "u1", state_after: "s0" }), run(1, 0, { unit: "u1", state_after: "s1" }), run(2, 0, { unit: "u2", state_after: "s2" }), run(3, 0, { unit: "u2" })],
    [
      event(0, 0, "deploy", "deployed version 1", { version: 1 }),
      event(1, 2, "deploy", "deployed version 2", { version: 2 }),
      event(2, 3, "lifecycle", "paused"),
      event(3, 4, "restore", "put back the state run 0 left, in session 2", { session: 2, from: "run 0" }),
      event(4, 4, "grant", "approved Clock"),
    ],
    [version(2, { current: true, unit: "u2" }), version(1, { unit: "u1" })],
  );
  const at = (key: string) => targetOf(timeline, timeline.findIndex((i) => i.key === key), "u2");

  it("goes back to a run or a version as itself", () => {
    expect(at("r1")).toMatchObject({ label: "run 1", code: { run: 1 }, state: { run: 1 }, why: { code: null, state: null } });
    expect(at("e0")).toMatchObject({ label: "version 1", code: { version: 1 }, state: { version: 1 } });
  });

  it("goes back to anything else as it was right after: the version then, the state then", () => {
    // The pause: version 2's code (deployed now), the state run 2 left (run 3 didn't commit one).
    expect(at("e2")).toMatchObject({ label: "event #2", code: { version: 2 }, state: { run: 2 }, why: { code: "the code it had then is the code deployed now", state: null } });
    // A restore: what it put back; a grant after it, the same.
    expect(at("e3")).toMatchObject({ state: { run: 0 }, why: { state: null } });
    expect(at("e4")).toMatchObject({ state: { run: 0 }, why: { state: null } });
  });

  it("says why when the state then isn't a point", () => {
    const fresh = buildTimeline([run(0, 0, { state_after: "s0" })], [event(0, 1, "lifecycle", "ran from fresh, in session 2", { session: 2 }), event(1, 1, "grant", "approved Clock")], []);
    expect(targetOf(fresh, 0, undefined).why.state).toMatch(/came from a stop \("ran from fresh/);
    expect(targetOf(buildTimeline([], [event(0, 0, "grant", "x")], []), 0, undefined).why.state).toBe("no run had left a state before it");
    expect(targetOf(buildTimeline([], [event(0, 0, "restore", "put back…")], []), 0, undefined).why.state).toMatch(/recorded before/);
    // Recorded before restores named it: read from its words.
    expect(targetOf(buildTimeline([], [event(0, 0, "restore", "put back the state version 4 left, in session 8")], []), 0, undefined).state).toEqual({ version: 4 });
  });

  it("restores code and state from one point together, from two one after the other", () => {
    expect(restoreParts(at("r1"), { code: true, state: true })).toEqual([{ point: { run: 1 }, what: { code: true, state: true } }]);
    const then = { label: "event #9", code: { version: 1 }, state: { run: 2 }, why: { code: null, state: null } };
    expect(restoreParts(then, { code: true, state: true })).toEqual([
      { point: { version: 1 }, what: { code: true, state: false } },
      { point: { run: 2 }, what: { code: false, state: true } },
    ]);
    expect(restoreParts(at("e2"), { code: true, state: true })).toEqual([{ point: { run: 2 }, what: { code: false, state: true } }]);
  });

  it("names the point a restore put back", () => {
    expect([parsePoint("version 3"), parsePoint("run 41"), parsePoint("session 2")]).toEqual([{ version: 3 }, { run: 41 }, null]);
  });

  it("says how it was then, and what each part does", () => {
    const then = { label: "event #2", event: { seq: 2, at_ms: 0, run_seq: 3, kind: "lifecycle", what: "paused" } };
    const said = restoreReport("prices", then, [{ point: { run: 2 }, what: { code: false, state: true }, preview: { code: null, state: { ...change0, from: "run 2" } } }]);
    expect(said.title).toBe('Restore "prices" to event #2: its state?');
    expect(said.sections[0]).toEqual({ heading: "as it was then", lines: ['As it was after "paused": the state run 2 left.'] });
  });
});

