import { describe, expect, it } from "vitest";
import { appended, clock, isDropped, levelName, logQuery } from "./logLines";
import type { LogLine } from "./server";

const line = (run: number, text: string, extra: Partial<LogLine> = {}): LogLine => ({ at: run, level: "log", unit: "u", run, text, ...extra });

describe("the log lines a deployment shows", () => {
  it("asks for a level, and for a run either up to its end or its own", () => {
    expect(logQuery("all", undefined, false, 500)).toEqual({ level: undefined, after: undefined, limit: 500 });
    expect(logQuery("warn", undefined, false, 500, 12)).toEqual({ level: "warn", after: 12, limit: 500 });
    expect(logQuery("all", 7, false, 500)).toEqual({ level: undefined, until: 7, after: undefined, limit: 500 });
    expect(logQuery("error", 7, true, 500)).toEqual({ level: "error", run: 7, after: undefined, limit: 500 });
  });

  it("writes a time to the millisecond and a level in capitals", () => {
    expect(clock(Date.now())).toMatch(/^\d\d:\d\d:\d\d\.\d{3}$/);
    expect(clock(new Date(2026, 10, 10, 9, 5, 3, 7).getTime())).toBe("09:05:03.007");
    expect(levelName(line(1, "x", { level: "warn" }))).toBe("WARN");
  });

  it("knows a line that stands for dropped ones", () => {
    expect(isDropped(line(1, "3 lines were dropped", { dropped: 3 }))).toBe(true);
    expect(isDropped(line(1, "kept"))).toBe(false);
  });

  it("keeps the newest lines when more are added", () => {
    const old = [line(1, "a"), line(2, "b"), line(3, "c")];
    expect(appended(old, [line(4, "d")], 10).map((l) => l.text)).toEqual(["a", "b", "c", "d"]);
    expect(appended(old, [line(4, "d"), line(5, "e")], 3).map((l) => l.text)).toEqual(["c", "d", "e"]);
  });
});
