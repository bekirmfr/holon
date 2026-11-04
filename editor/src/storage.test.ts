import { describe, expect, it } from "vitest";
import { cacheLine, diskLine, isEvery, keptLines, reportLine, scheduleLine, sizeText, span } from "./storage";

const at = 1_000_000_000_000;
const report = { at_ms: at, took_ms: 50_500, removed: 137962, bytes: 60_293_120, skipped: false, error: null };

describe("the server's sweeps, as the admin page says them", () => {
  it("says what the last sweep did, and how long ago", () => {
    expect(reportLine(report, "values", at + 3 * 60_000)).toBe("3 min ago: 137962 values, 57.5 MiB freed in 51 s");
    expect(reportLine({ ...report, removed: 0 }, "values", at + 30_000)).toBe("30 s ago: nothing to remove (51 s)");
    expect(reportLine({ ...report, skipped: true }, "values", at)).toBe("0 s ago: nothing new to look at");
    expect(reportLine({ ...report, error: "disk gone" }, "values", at)).toBe("0 s ago, failed: disk gone");
    expect(reportLine(null, "values", at)).toBe("not swept yet");
  });
  it("says how often, and when next", () => {
    const sweeps = { every: "10m", next_at_ms: at + 7 * 60_000, sweeping: false, store: null, folders: null };
    expect(scheduleLine(sweeps, at)).toBe("every 10m; the next in 7 min");
    expect(scheduleLine({ ...sweeps, sweeping: true }, at)).toBe("every 10m; sweeping now");
    expect(scheduleLine({ ...sweeps, every: "off", next_at_ms: null }, at)).toContain("doesn't sweep on its own");
  });
  it("takes what the server takes", () => {
    for (const t of ["90s", "10m", "2h", "1d", "off", " 30m "]) expect(isEvery(t)).toBe(true);
    for (const t of ["5s", "soon", "10", "1w", ""]) expect(isEvery(t)).toBe(false);
  });
  it("says spans and sizes roughly", () => {
    expect([span(500), span(120_000), span(3 * 3_600_000), span(3 * 86_400_000)]).toEqual(["1 s", "2 min", "3 h", "3 days"]);
    expect([sizeText(2048), sizeText(60_293_120)]).toEqual(["2 KiB", "57.5 MiB"]);
  });
});

describe("what the host holds, as the admin page says it", () => {
  it("says the caches and the disks against their sizes", () => {
    expect(cacheLine({ entries: 12, bytes: 32 << 20, budget: 256 << 20 }, "flows")).toBe("12 flows, 32 MiB of 256 MiB (12%)");
    expect(diskLine({ path: "/data", holds: ["data", "logs", "store"], size: 100 * 2 ** 30, free: 40 * 2 ** 30, used: 60 * 2 ** 30 })).toBe(
      "60% full: 40 GiB free of 100 GiB (60 GiB in use): data, logs, store",
    );
  });
  it("says what it keeps, and what crashes left", () => {
    const none = { files: 0, bytes: 0, on_disk: 0 };
    const kept = {
      at_ms: at,
      store: { values: 432808, bytes: 2 ** 30, on_disk: 2 * 2 ** 30, put_since: 3200 },
      logs: { files: 34, bytes: 192 << 20, on_disk: 192 << 20 },
      file_texts: null,
      leftovers: { removed: { files: 3, bytes: 62 << 20, on_disk: 62 << 20 }, waiting: none },
    };
    expect(keptLines(kept, at + 180_000)).toEqual([
      "counted 3 min ago",
      "the store: 432808 values, 2 GiB on disk (1 GiB of bytes); 3200 put since",
      "logs: 34, 192 MiB on disk",
      "left by crashes: 3 files removed (62 MiB), 0 still to remove",
    ]);
    expect(keptLines({ ...kept, at_ms: null, store: null, logs: null, leftovers: { removed: none, waiting: none } }, at)[0]).toContain("not counted yet");
  });
});
