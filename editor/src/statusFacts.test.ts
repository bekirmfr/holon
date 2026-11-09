import { describe, expect, it } from "vitest";
import { lastDay, sinceOf } from "./statusFacts";
import type { LogEntry } from "./server";

const run = (at_ms: number, duration_ms: number, error: string | null = null) => ({ at_ms, duration_ms, error, status: 200 }) as LogEntry;

describe("statusFacts", () => {
  it("counts the last day's runs, its failures and how long they took", () => {
    const now = 100 * 24 * 3600 * 1000;
    const day = lastDay([run(now - 25 * 3600 * 1000, 99), run(now - 10, 1), run(now - 5, 3, "boom"), run(now - 1, 2)], now);
    expect(day).toEqual({ runs: 3, failed: 1, p50: 2, p95: 3 });
    expect(lastDay([], now)).toEqual({ runs: 0, failed: 0, p50: null, p95: null });
  });

  it("says since which version each thing has been asked for, and who deployed it", () => {
    const versions = [
      { number: 3, by: "ada", items: ["A", "B"] },
      { number: 2, by: "cy", items: ["A", "B"] },
      { number: 1, by: "ada", items: ["A"] },
    ];
    const since = sinceOf(["A", "B"], versions);
    expect(since.get("A")).toEqual({ since: 1, by: "ada", known: true });
    expect(since.get("B")).toEqual({ since: 2, by: "cy", known: true });
  });

  it("says a version it can't read stops the walk: since then at least", () => {
    const since = sinceOf(["A"], [
      { number: 3, by: "ada", items: ["A"] },
      { number: 2, by: null, items: null },
    ]);
    expect(since.get("A")).toEqual({ since: 3, by: "ada", known: false });
    expect(sinceOf(["A"], [{ number: 1, by: null, items: null }]).get("A")).toEqual({ since: 1, by: null, known: false });
  });
});
