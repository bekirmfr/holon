// People's Last seen and Last used (lastSeen.ts): never, just now, how long ago, the exact time as
// the title.

import { describe, expect, it } from "vitest";
import { seenSaid } from "./lastSeen";

describe("last seen and last used", () => {
  const now = Date.UTC(2026, 10, 12, 12, 0, 0);
  const at = (ms: number) => (now - ms) / 1000;

  it("says never when nothing came from it", () => {
    expect(seenSaid(undefined, now).text).toBe("never");
    expect(seenSaid(null, now).text).toBe("never");
  });

  it("says how long ago, the exact time as its title", () => {
    expect(seenSaid(at(20_000), now).text).toBe("just now");
    expect(seenSaid(at(-5_000), now).text).toBe("just now");
    expect(seenSaid(at(7 * 60_000), now).text).toBe("7 min ago");
    expect(seenSaid(at(3 * 3_600_000), now).text).toBe("3 h ago");
    expect(seenSaid(at(4 * 86_400_000), now).text).toBe("4 days ago");
    expect(seenSaid(at(7 * 60_000), now).title).toBe(new Date(now - 7 * 60_000).toLocaleString());
  });
});
