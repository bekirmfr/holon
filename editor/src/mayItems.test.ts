// The It may strip's diff (mayItems.ts): the draft's items beside the deployed version's.

import { describe, expect, it } from "vitest";
import { mayDiff } from "./mayItems";

describe("mayDiff", () => {
  it("puts the new first, then the same, then what the draft no longer asks for", () => {
    expect(mayDiff(["NetworkIngress", 'SecretRead("k")', "Clock"], ["Clock", "NetworkIngress", 'CollectionRead("p")'])).toEqual([
      { item: 'SecretRead("k")', change: "new" },
      { item: "NetworkIngress", change: "same" },
      { item: "Clock", change: "same" },
      { item: 'CollectionRead("p")', change: "gone" },
    ]);
  });

  it("marks nothing new for a flow not deployed", () => {
    expect(mayDiff(["NetworkIngress"], null)).toEqual([{ item: "NetworkIngress", change: "same" }]);
    expect(mayDiff([], [])).toEqual([]);
  });
});
