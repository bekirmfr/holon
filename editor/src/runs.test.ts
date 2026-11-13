import { describe, expect, it } from "vitest";
import { outside, runLabel, runOutcome, startedBy } from "./runs";
import type { LogEntry } from "./server";

const run = (over: Partial<LogEntry>): LogEntry => ({
  seq: 1,
  session: 1,
  at_ms: 0,
  duration_ms: 1,
  kind: "request",
  route: "GET /a",
  method: "GET",
  url: "/a",
  status: 200,
  request: null,
  sourced: [],
  state_before: {},
  outputs: [],
  performed: [],
  response_body: null,
  error: null,
  ...over,
});

describe("runs", () => {
  it("says how a run ended: an error or a 5xx failed, a parked one waits", () => {
    expect(runOutcome(run({}))).toBe("ok");
    expect(runOutcome(run({ status: 404 }))).toBe("ok");
    expect(runOutcome(run({ status: 503 }))).toBe("failed");
    expect(runOutcome(run({ status: null, error: "boom" }))).toBe("failed");
    expect(runOutcome(run({ parked: ["approval"] }))).toBe("waiting");
  });

  it("counts what it did outside: the requests it sent and the effects it performed", () => {
    const req = { type: "Null" } as never;
    const r = run({ rounds: [{ exchanges: [{ request: req, response: req }, { request: req, response: req }] }] as never, performed: [{ capability: "Db" }] as never });
    expect(outside(r)).toEqual({ sent: 2, did: 1 });
    expect(outside(run({}))).toEqual({ sent: 0, did: 0 });
  });

  it("labels a run by its route, or what started it", () => {
    expect(runLabel(run({}))).toBe("GET /a");
    expect(runLabel(run({ route: null }))).toBe("GET /a (no route)");
    expect(runLabel(run({ kind: "tick", route: null }))).toBe("clock tick");
  });

  it("says who started a run: whoever ran it through the server, else what does on its own", () => {
    expect(startedBy("trigger", "ada")).toBe("ada");
    expect(startedBy("request", "claude")).toBe("claude");
    expect(startedBy("request", undefined)).toBe("a request on its port");
    expect(startedBy("tick", null)).toBe("its clock");
    expect(startedBy("trigger", null)).toBe("its action (who ran it isn't on record)");
  });
});
