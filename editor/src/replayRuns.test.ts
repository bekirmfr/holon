import { describe, expect, it } from "vitest";
import { replaySummary, whyOf } from "./ReplayRuns";
import type { Replayed } from "./server";

const replayed = (same: number, differ: number, cant_tell: number): Replayed => ({ unit: "u", replayed: same + differ + cant_tell, same, differ, cant_tell, runs: [] });

describe("a replay on recent runs, as its card says it", () => {
  it("counts what was replayed, leaving out what is none", () => {
    expect(replaySummary(replayed(17, 3, 0))).toBe("20 replayed · 17 same · 3 differ");
    expect(replaySummary(replayed(2, 1, 1))).toBe("4 replayed · 2 same · 1 differs · 1 can't tell");
    expect(replaySummary(replayed(3, 0, 0))).toBe("3 replayed · 3 same");
    expect(replaySummary(replayed(0, 0, 0))).toBe("no runs to replay yet");
  });
  it("says why a run differs in one line", () => {
    expect(whyOf({ seq: 2, at_ms: 0, kind: "request", outcome: "differs", why: ["it answers 422 where it answered 200", "its answer"] })).toBe("it answers 422 where it answered 200; its answer");
    expect(whyOf({ seq: 1, at_ms: 0, kind: "tick", outcome: "same" })).toBe("");
  });
});
