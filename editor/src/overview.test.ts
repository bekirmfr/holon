import { describe, expect, it } from "vitest";
import { bars, chainSaid, failedShare, greeting, hostLine, hoursSaid, liveSaid } from "./overview";

describe("the overview's words", () => {
  it("say the share that failed, and none with no runs", () => {
    expect(failedShare(0, 0)).toBe("–");
    expect(failedShare(14288, 0)).toBe("0%");
    expect(failedShare(1000, 2)).toBe("0.2%");
    expect(failedShare(100000, 2)).toBe("<0.1%");
    expect(failedShare(8, 2)).toBe("25%");
    expect(failedShare(3, 1)).toBe("33%");
  });

  it("say which flows are live and which are held", () => {
    expect(liveSaid({ runs: 0, failed: 0, running: 3, paused: 0, stopped: 0 })).toBe("flows live");
    expect(liveSaid({ runs: 0, failed: 0, running: 1, paused: 1, stopped: 0 })).toBe("flow live, 1 paused");
    expect(liveSaid({ runs: 0, failed: 0, running: 2, paused: 1, stopped: 1 })).toBe("flows live, 2 paused or stopped");
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
    expect(hostLine(1, 1, 34)).toBe("1 flow on this host. One thing waits for approval. 34 runs wait for an event.");
  });
});
