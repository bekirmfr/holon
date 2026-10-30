import { describe, expect, it } from "vitest";
import { buildTimeline, fileDiff, fileLine, filterOf, itemText, parsePoint, restoreDiffs, restoreReport, restoreParts, restoredLine, runOptions, stateDiff, stateLines, targetOf, versionOptions } from "./timeline";
import type { DeploymentEvent, FileSince, LogEntry, VersionEntry } from "./server";

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
      "The state version 1 left is put back: it changes the collection prices; drops note.",
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

  it("says what it changes, a line each: the code, the grant, the state", () => {
    const state = { from: "version 1", changed: ["total"], added: ["cart"], removed: ["seen", "last"], runs_since: 2, effects_since: {} };
    const preview = { code: 1, needs_approval: ["Clock"], missing_secrets: [], items: ["Clock", "NetworkIngress"], granted_now: ["NetworkIngress", 'StateWrite("x")'], state };
    const asked = restoreReport("prices", { label: "version 1" }, [{ point: { version: 1 }, what: { code: true, state: true }, preview }], (i: string) => i, [version(2, { current: true }), version(1)]);
    expect(asked.changes).toEqual([
      "code: version 1's is deployed again, as version 3, in place of version 2",
      "permissions: 1 added, 1 dropped, 1 kept",
      "state: as version 1 left it, 1 key changed, 1 key brought back, 2 keys dropped",
    ]);
    const same = restoreReport("prices", { label: "run 7" }, [{ point: { run: 7 }, what: { code: false, state: true }, preview: { code: null, state: { ...state, changed: [], added: [], removed: [] } } }]);
    expect(same.changes).toEqual(["state: as version 1 left it, the same as now"]);
  });

  it("diffs the code deployed now with the restored version's, the grant, and each state key", async () => {
    const values = { total: { now: { type: "Number" as const, value: 40 }, then: { type: "Number" as const, value: 4.5 } }, seen: { now: { type: "Boolean" as const, value: true }, then: null } };
    const state = { from: "version 1", changed: ["total"], added: [], removed: ["seen"], runs_since: 1, effects_since: {}, values };
    const preview = { code: 1, items: ["Clock"], granted_now: ["Clock", "NetworkEgress"], withheld: [], state };
    const texts: Record<string, string> = { u1: "flow p\n  a = 1\n", u2: "flow p\n  a = 2\n" };
    const diffs = await restoreDiffs([{ point: { version: 1 }, what: { code: true, state: true }, preview }], [version(2, { current: true }), version(1)], async (u) => ({
      plain: texts[u],
      named: texts[u],
    }));
    expect(diffs.map((d) => [d.title, d.legend])).toEqual([
      ["code", "− version 2, deployed now   + version 1"],
      ["permissions", "− granted now, dropped   + added   kept"],
      ["state", "− now   + as version 1 left it"],
    ]);
    expect(diffs[0].lines.map((l) => l.sign + l.text)).toEqual([" flow p", "-  a = 2", "+  a = 1", " "]);
    expect(diffs[1].lines.map((l) => l.sign + l.text)).toEqual([" Clock", "-NetworkEgress"]);
    expect(diffs[2].lines.map((l) => l.sign + l.text)).toEqual(["-seen: true", "-total: 40", "+total: 4.5"]);
    // A version whose text can't be had: said, not a failure.
    const missing = await restoreDiffs([{ point: { version: 1 }, what: { code: true, state: false }, preview: { code: 1 } }], [version(2, { current: true }), version(1)], async () => {
      throw new Error("not kept");
    });
    expect(missing[0]).toMatchObject({ title: "code", lines: [], note: "The text can't be compared: not kept" });
  });

  it("compares two versions by their text's names only when both have them", async () => {
    // Version 2 deployed from text (`fee`), version 1 from the canvas (no names: `c`).
    const texts: Record<string, { plain: string; named?: string }> = {
      u2: { plain: "c = 5\nrespond c", named: "fee = 5\nrespond fee" },
      u1: { plain: "c = 0.5\nrespond c" },
    };
    const [code] = await restoreDiffs([{ point: { version: 1 }, what: { code: true, state: false }, preview: { code: 1 } }], [version(2, { current: true }), version(1)], async (u) => texts[u]);
    expect(code.lines.map((l) => l.sign + l.text)).toEqual(["-c = 5", "+c = 0.5", " respond c"]);
    expect(code.note).toBe("Printed without the text's names: version 1 was deployed from the canvas, which keeps none.");
  });

  it("says when a state diff leaves keys out", () => {
    const d = stateDiff({ from: "run 3", changed: ["a", "b"], added: [], removed: [], runs_since: 0, effects_since: {}, values: { a: { now: { type: "Number", value: 1 }, then: { type: "Number", value: 2 } } } });
    expect(d.note).toBe("1 more key(s) change: their values aren't shown.");
    expect(stateDiff({ from: "run 3", changed: [], added: [], removed: [], runs_since: 0, effects_since: {} }).note).toBe("No key changes.");
  });

  it("shows a collection's items that differ, as the flow reads them, not its root", () => {
    const n = (value: number) => ({ type: "Number" as const, value });
    const prices = { items: { tea: { now: n(40), then: n(4) }, cake: { now: null, then: n(3) } }, more: false };
    const change = { from: "version 1", changed: ["#collection:prices"], added: [], removed: [], runs_since: 1, effects_since: {}, values: {}, collections: { prices } };
    const d = stateDiff(change);
    expect(d.lines.map((l) => l.sign + l.text)).toEqual(['-prices["tea"]: 40', '+prices["cake"]: 3', '+prices["tea"]: 4']);
    expect(d.note).toBeUndefined();
    const report = restoreReport("p", { label: "version 1" }, [{ point: { version: 1 }, what: { code: false, state: true }, preview: { code: null, state: change } }]);
    expect(report.changes).toEqual(["state: as version 1 left it, 1 key changed (prices: 2 items)"]);
    expect(stateDiff({ ...change, collections: { prices: { ...prices, more: true } } }).note).toBe("Only some of prices's items are compared or shown.");
  });

  it("names the files the runs since wrote, which it leaves as they are, and an edit outside a run", async () => {
    const h = (c: string) => c.repeat(64);
    const sheet: FileSince = {
      folder: "sheets",
      name: "changes.txt",
      writes: 3,
      runs: [5, 6, 7],
      last: { kind: "write", hash: h("b"), size: 20, run: 7 },
      at_point: { kind: "write", hash: h("a"), size: 7, run: 4 },
      disk: { hash: h("c"), size: 30 },
      unreadable: null,
      outside: true,
      then_text: "tea: 4\n",
      now_text: "tea: 4\ntea: 40\nby hand\n",
    };
    expect(fileLine(sheet)).toBe("sheets/changes.txt: 3 writes since, by runs 5, 6, 7 (the last written); changed outside a run since run 7: on disk now #cccccccc, not what run 7 left");
    expect(fileLine({ ...sheet, writes: 1, runs: [7], outside: false })).toBe("sheets/changes.txt: 1 write since, by run 7 (the last written)");
    expect(fileLine({ ...sheet, disk: null })).toMatch(/; gone from disk since run 7, which left it$/);
    expect(fileLine({ ...sheet, unreadable: "no folder sheets" })).toMatch(/; can't be checked on disk now: no folder sheets$/);

    const state = { from: "run 4", changed: ["#collection:stock"], added: [], removed: [], runs_since: 3, effects_since: { FileWrite: 3 }, values: {}, files: [sheet] };
    const report = restoreReport("stock-sheet", { label: "run 4" }, [{ point: { run: 4 }, what: { code: false, state: true }, preview: { code: null, state } }]);
    expect(report.changes).toContain("files: left as they are (1 written since, 1 of them changed outside a run)");
    expect(report.sections.map((s) => s.heading)).toEqual(["state", "files, not put back"]);

    // Its diff: how it changed since, then beside now; a restore leaves it so.
    const d = fileDiff(sheet, "run 4");
    expect([d.title, d.legend]).toEqual(["file sheets/changes.txt", "− at run 4   + now on disk"]);
    expect(d.lines.map((l) => l.sign + l.text)).toEqual([" tea: 4", "+tea: 40", "+by hand"]);
    expect(d.note).toBe("A restore leaves it as it is now. Changed outside a run since run 7.");
    const diffs = await restoreDiffs([{ point: { run: 4 }, what: { code: false, state: true }, preview: { code: null, state } }], [], async () => ({ plain: "" }));
    expect(diffs.map((x) => x.title)).toEqual(["state", "file sheets/changes.txt"]);
    // Its text then not in the log: by hash. No run wrote it before: unknown. Gone: all removed.
    expect(fileDiff({ ...sheet, then_text: null }, "run 4")).toMatchObject({ lines: [], note: expect.stringContaining("isn't in the log (no run read it): #aaaaaaaa then, #cccccccc now.") });
    expect(fileDiff({ ...sheet, at_point: null }, "run 4").note).toContain("No run wrote it before run 4");
    expect(fileDiff({ ...sheet, disk: null, now_text: null }, "run 4").lines.map((l) => l.sign + l.text)).toEqual(["-tea: 4"]);
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

