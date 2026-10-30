import { describe, expect, it } from "vitest";
import { buildTimeline, filterOf, restoreMessage, restoredLine, runOptions, stateLines, versionOptions } from "./timeline";
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
    const asked = restoreMessage("prices", { version: 1 }, { code: true, state: true }, { code: 1, needs_approval: ["Clock"], missing_secrets: [], state: change }, (i) => `read ${i}`);
    expect(asked.action).toBe("Approve and restore");
    expect(asked.lines[0]).toBe('Restore "prices" to version 1: its code and state?');
    expect(asked.lines).toContain("It needs your approval to:");
    expect(asked.lines).toContain("• read Clock");
    const plain = restoreMessage("prices", { run: 7 }, { code: false, state: true }, { code: null, state: change });
    expect(plain.action).toBe("Restore");
    expect(plain.lines.join("\n")).not.toMatch(/code/);
  });

  it("says what it did", () => {
    expect(restoredLine({ version: 1 }, { code: 1, version: 4, state: { from: "version 1", session: 3 } })).toBe(
      "restored: version 1's code is deployed again, as version 4; the state version 1 left is live again, in session 3 (session 2 keeps the one it replaced)",
    );
  });
});
