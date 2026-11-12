import { describe, expect, it } from "vitest";
import { bars, chainSaid, failedShare, greeting, hostLine, hoursSaid, lastRun, liveOf, tookSaid } from "./overview";
import type { Summary } from "./server";

describe("the overview's words", () => {
  it("say the share that failed, and none with no runs", () => {
    expect(failedShare(0, 0)).toBe("–");
    expect(failedShare(14288, 0)).toBe("0%");
    expect(failedShare(1000, 2)).toBe("0.2%");
    expect(failedShare(100000, 2)).toBe("<0.1%");
    expect(failedShare(8, 2)).toBe("25%");
    expect(failedShare(3, 1)).toBe("33%");
  });

  it("say how many flows are live of all deployed", () => {
    expect(liveOf({ runs: 0, failed: 0, running: 3, paused: 0, stopped: 0 })).toBe("3 of 3");
    expect(liveOf({ runs: 0, failed: 0, running: 7, paused: 1, stopped: 1 })).toBe("7 of 9");
  });

  it("say how long a run took", () => {
    expect([0.42, 12.4, 999.6, 1340, 125_000].map(tookSaid)).toEqual(["0.4 ms", "12 ms", "1.0 s", "1.3 s", "2 min"]);
    expect(tookSaid(null)).toBe("–");
    expect(tookSaid(undefined)).toBe("–");
  });

  it("say when a flow last ran: its newest run's start, though not among the latest nor in the window", () => {
    const flow = (slug: string, runs: number[], last_ms: number | null) => ({ slug, status: "running" as const, runs, failed: runs.map(() => 0), p50_ms: null, last_ms, unrecorded: { runs: 0, failed: 0 }, on_record: 0, head: "", breaks: [] });
    const s: Summary = {
      at_ms: 0,
      hours: 4,
      totals: { runs: 0, failed: 0, running: 3, paused: 0, stopped: 0 },
      deployments: [flow("a", [0, 0, 0, 2], 1234), flow("b", [1, 3, 0, 0], 99), flow("c", [0, 0, 0, 0], 5), flow("d", [0, 0, 0, 0], null)],
      latest: [{ slug: "a", seq: 7, at_ms: 1234, duration_ms: 1, kind: "request", route: "GET /", status: 200, failed: false }],
      chain: { on_record: 0, breaks: 0, witness: null, checked: null },
    };
    expect(["a", "b", "c", "d", "e"].map((slug) => lastRun(slug, s))).toEqual([1234, 99, 5, null, null]);
  });

  it("say the run log by its last check, else as it was written", () => {
    const time = (ms: number) => `t${ms}`;
    const chain = { on_record: 10, breaks: 0, witness: null, checked: null };
    expect(chainSaid(chain, time)).toEqual({ value: "intact", label: "run log, as written (not checked yet)", bad: false });
    const checked = { at_ms: 5, runs: 10, breaks: [{ slug: "a", seq: 3 }], heads: [] };
    expect(chainSaid({ ...chain, checked }, time)).toEqual({ value: "1 break", label: "run log, checked t5", bad: true });
  });

  it("draw each hour against the busiest, red where one failed", () => {
    expect(bars([0, 10, 1], [0, 0, 1])).toEqual([
      { height: 0, bad: false },
      { height: 1, bad: false },
      { height: 0.1, bad: true },
    ]);
    expect(bars([0, 100, 1], [0, 0, 0])[2].height).toBe(0.08);
    expect(hoursSaid([1, 2, 0], [0, 1, 0])).toBe("3 runs, 1 failed in the last 3 hours");
  });

  it("greet by the hour and sum up the host", () => {
    expect([9, 13, 20].map(greeting)).toEqual(["Good morning", "Good afternoon", "Good evening"]);
    expect(hostLine(4, 0, 0)).toBe("4 flows on this host. Nothing waits for approval.");
    expect(hostLine(1, 1, 34)).toBe("1 flow on this host. One thing waits for approval. 34 runs wait for an approval or an event.");
  });
});
