import { describe, expect, it } from "vitest";
import { tallyLine, tallyRows } from "./tallies";

describe("unrecorded handlers' tallies", () => {
  it("says how many ran, how many failed, how long each took and when the last did", () => {
    const at = (ms: number) => `t${ms}`;
    expect(tallyLine({ runs: 1204, failed: 2, total_ms: 481.6, last_at_ms: 99, last_failure: { at_ms: 70, status: 503, error: "the store didn't answer" } }, at)).toBe(
      "1,204 runs, 2 failed (the last 503 at t70: the store didn't answer), 0.4 ms each, the last at t99",
    );
    expect(tallyLine({ runs: 3, failed: 1, total_ms: 3, last_at_ms: 9, last_failure: { at_ms: 9, status: 500 } }, at)).toBe("3 runs, 1 failed (the last 500 at t9), 1.0 ms each, the last at t9");
    expect(tallyLine({ runs: 1, failed: 0, total_ms: 2, last_at_ms: 5 }, at)).toBe("1 run, 2.0 ms each, the last at t5");
    expect(tallyLine({ runs: 0, failed: 0, total_ms: 0, last_at_ms: 0 }, at)).toBe("0 runs");
  });

  it("lists one row per handler, by label, and none from a server without tallies", () => {
    const t = { runs: 1, failed: 0, total_ms: 1, last_at_ms: 0 };
    expect(tallyRows({ "GET /health": t, "action report": t }).map((r) => r.label)).toEqual(["GET /health", "action report"]);
    expect(tallyRows(undefined)).toEqual([]);
  });
});
