// What a flow's page says of it (flowFacts.ts): what starts it, its description, Run now's
// choices, what a run sent out, its secrets and where they went, witnessed or not, the files its
// runs wrote, its limits and how a typed one applies, and a cell's values over time.

import { describe, expect, it } from "vitest";
import { cellPoints, filesWritten, firstComment, limitChange, limitRows, runNowOptions, secretUses, startsSaid, wentOut, witnessed } from "./flowFacts";
import type { WValue } from "./holon";
import type { CellHistory, Grant, LogEntry } from "./server";

const run = (seq: number, extra: Partial<LogEntry> = {}): LogEntry => ({
  seq,
  session: 1,
  at_ms: 1000,
  duration_ms: 2,
  kind: "request",
  route: "GET /hello/:name",
  method: "GET",
  url: "/hello/Ada",
  status: 200,
  request: null,
  sourced: [],
  state_before: {},
  outputs: [],
  performed: [],
  response_body: null,
  error: null,
  ...extra,
});
const text = (value: string): WValue => ({ type: "Text", value });
const record = (fields: [string, WValue][]): WValue => ({ type: "Record", value: fields });

describe("the header", () => {
  it("says what starts it: its routes and port, then its other triggers", () => {
    expect(startsSaid(["Http :8110"], 8110, 2)).toBe("2 routes · :8110");
    expect(startsSaid(["Http :8110", "every 10s", "action reset"], 8110, 1)).toBe("1 route · :8110 · every 10s · action reset");
    expect(startsSaid(["every 1m"], null, null)).toBe("every 1m");
    expect(startsSaid([], null, null)).toBe("nothing starts it");
  });

  it("takes its description from the first comment line of its text", () => {
    expect(firstComment("\n// Says hello to whoever asks.\n// More.\nexport const x = 1")).toBe("Says hello to whoever asks.");
    expect(firstComment("export const x = 1 // not this")).toBeNull();
    expect(firstComment(undefined)).toBeNull();
  });

  it("runs now a clock or an action, nothing else", () => {
    expect(runNowOptions(["Http :8110", "every 10s", "on file inbox", "action reset"])).toEqual([
      { kind: "clock", name: "every 10s" },
      { kind: "action", name: "reset" },
    ]);
    expect(runNowOptions(["Http :8110"])).toEqual([]);
  });
});

describe("a run", () => {
  it("says what went out: its answer, else what it produced", () => {
    expect(wentOut(run(1, { response_body: '{"hello":\n "Ada"}' }))).toBe('{"hello": "Ada"}');
    expect(wentOut(run(1, { response_body: "" }))).toBe("(empty)");
    expect(wentOut(run(1, { response_body: "x".repeat(80) }))).toHaveLength(60);
    expect(wentOut(run(1, { outputs: [text("done")] }))).toContain("done");
    expect(wentOut(run(1))).toBe("");
  });

  it("names each secret it read by version and fingerprint, and the hosts its mark went to", () => {
    const exchange = (url: string, auth: string) => ({ path: [0], request: record([["url", text(url)], ["headers", record([["authorization", text(auth)]])]]), response: null as unknown as WValue, start_us: 0, end_us: 1 });
    const e = run(1, {
      secrets: { api_key: "v3 #a1b2c3d4", unused: "v1 #00ff00ff" },
      rounds: [{ exchanges: [exchange("https://api.example.com/v1/x", "Bearer ‹secret:api_key v3 #a1b2c3d4›"), exchange("https://other.example/y", "none")] }] as LogEntry["rounds"],
    });
    expect(secretUses(e)).toEqual([
      { name: "api_key", version: "v3", fingerprint: "#a1b2c3d4", to: ["api.example.com"] },
      { name: "unused", version: "v1", fingerprint: "#00ff00ff", to: [] },
    ]);
  });

  it("is witnessed once a batch was taken at or after it ended", () => {
    expect(witnessed(1000, null).yes).toBe(false);
    expect(witnessed(1000, null).said).toContain("--export-heads");
    expect(witnessed(1000, { address: "https://w.example", taken_ms: 2000, waiting: 0 })).toMatchObject({ yes: true });
    const not = witnessed(3000, { address: "https://w.example", taken_ms: 2000, waiting: 2 });
    expect(not.yes).toBe(false);
    expect(not.said).toContain("2 heads wait");
  });
});

describe("the files its runs wrote", () => {
  it("counts the runs that wrote each and keeps the last, leaving out failed runs", () => {
    const write = (folder: string, name: string): LogEntry["performed"][number] => ({ capability: `FileWrite("${folder}")`, payload: record([["name", text(name)], ["size", { type: "Number", value: 3 }]]) });
    const entries = [
      run(1, { performed: [write("out", "a.csv"), write("out", "a.csv")] }),
      run(2, { performed: [write("out", "b.csv")] }),
      run(3, { performed: [write("out", "a.csv")] }),
      run(4, { performed: [{ capability: 'FileWrite("out")', payload: text("b.csv") }] }),
      run(5, { performed: [write("out", "c.csv")], error: "failed" }),
    ];
    expect(filesWritten(entries)).toEqual([
      { folder: "out", name: "b.csv", runs: 2, last: 4, kind: "delete" },
      { folder: "out", name: "a.csv", runs: 2, last: 3, kind: "write" },
    ]);
  });
});

describe("its settings", () => {
  const grant = (extra: Partial<Grant> = {}): Grant => ({
    manifest: "m",
    items: ["Limit(run_budget_ms=9000)"],
    requirements: { limits: { run_budget_ms: 9000, max_redirects: 2 } },
    granted_at: 0,
    by: "deploy",
    ...extra,
  });
  const defaults = { max_requests: 64, run_budget_ms: 5000, max_request_bytes: 1000, keep_runs: 500, keep_run_days: 3650, max_redirects: 5 };

  it("holds each limit to what it asked for if granted, else the default, then lowered, then the ceiling", () => {
    const rows = limitRows(grant({ lowered: { keep_runs: 50 } }), defaults, { max_requests: 32 });
    expect(rows.map((r) => [r.name, r.now])).toEqual([
      ["max_requests", 32],
      ["run_budget_ms", 9000],
      ["max_request_bytes", 1000],
      ["keep_runs", 50],
      ["keep_run_days", 3650],
      ["max_redirects", 5],
    ]);
    expect(rows.find((r) => r.name === "keep_runs")?.group).toBe("runs kept");
    // Asked for but not granted: the default holds.
    expect(rows.find((r) => r.name === "max_redirects")).toMatchObject({ asked: 2, base: 5 });
    // A host that doesn't say its defaults: none shown.
    expect(limitRows(grant(), {}, {})).toEqual([]);
  });

  it("lowers at once, and raises only by a new version", () => {
    const [row] = limitRows(grant(), defaults, {}).filter((r) => r.name === "run_budget_ms");
    expect(limitChange(row, "1000")).toBe("lower");
    expect(limitChange(row, "0")).toBe("lower");
    expect(limitChange(row, "20000")).toBe("raise");
    expect(limitChange(row, "9000")).toBe("same");
    expect(limitChange(row, "")).toBe("same");
    expect(limitChange(row, "1.5")).toBe("bad");
    expect(limitChange(row, "-1")).toBe("bad");
  });
});

describe("a cell over time", () => {
  it("is its values after each change, oldest first, a gap where one isn't a number", () => {
    const n = (value: number): WValue => ({ type: "Number", value });
    const h: CellHistory = {
      name: "tea",
      item: null,
      collection: false,
      now: n(3),
      more: false,
      changes: [
        { seq: 9, at_ms: 3, before: text("x"), after: n(3) },
        { seq: 5, at_ms: 2, before: n(40), after: text("x") },
        { seq: 2, at_ms: 1, before: null, after: n(40) },
      ] as CellHistory["changes"],
    };
    expect(cellPoints(h).map((p) => [p.seq, p.value])).toEqual([
      [2, 40],
      [5, null],
      [9, 3],
    ]);
  });
});
