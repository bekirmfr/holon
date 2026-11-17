import { describe, expect, it } from "vitest";
import { tickSaid } from "./server";

describe("what the editor says of a tick it ran now", () => {
  it("counts what it did, or says it was left out and what would give it (R57)", () => {
    expect(tickSaid("every 2s", { at_epoch_seconds: 1, label: "every 2s", performed: [] })).toBe("every 2s: ticked, 0 effects performed");
    expect(tickSaid("every 2s", { at_epoch_seconds: 1, performed: [{}] })).toBe("every 2s: ticked, 1 effect performed");
    const left_out = "its outputs and what it did are its run's values, left out: rob may not read \"kept\"'s runs' steps and values";
    expect(tickSaid("every 1h", { at_epoch_seconds: 1, label: "every 1h", left_out })).toBe(`every 1h: ticked; ${left_out}`);
  });
});
