import { describe, expect, it } from "vitest";
import { touchLines, touchText, type Touch, type TouchUse } from "./touches";

const use = (item: string, what: string, change: TouchUse["change"], approval = false): TouchUse => ({ item, what, words: item, change, approval, optional: false });

describe("what a flow touches", () => {
  // The rows crates/holon/src/touches.rs's test builds, said as `holon editor` and the CLI say them.
  it("says each thing by group: what changes, what isn't ready, what a restore does", () => {
    const rows: Touch[] = [
      { group: "triggers", kind: "requests", name: "", change: "same", uses: [use("NetworkIngress", "receive", "same")], ready: null, has: null },
      {
        group: "resources",
        kind: "folder",
        name: "inbox",
        change: "changed",
        uses: [use('FileRead("inbox")', "read", "same"), use('FileWrite("inbox")', "write", "new", true)],
        ready: "not bound on this server",
        has: null,
      },
      { group: "resources", kind: "secret", name: "api_key", change: "same", uses: [use('SecretRead("api_key")', "read", "same")], ready: null, has: "set, v1 (a value kept here)" },
      { group: "resources", kind: "secret", name: "other", change: "new", uses: [use('SecretRead("other")', "read", "new")], ready: "not set: the steps that read it get nothing until it is", has: null },
      { group: "outside", kind: "host", name: "a.example", change: "gone", uses: [use('HttpRequest("a.example")', "call", "gone")], ready: null, has: null },
      { group: "outside", kind: "host", name: "b.example", change: "new", uses: [use('HttpRequest("b.example")', "call", "new", true)], ready: null, has: null, since: 2 },
      { group: "state", kind: "key", name: "count", change: "same", uses: [use('StateWrite("count")', "write", "same")], ready: null, has: null, restores: "changes" },
      { group: "other", kind: "randomness", name: "", change: "same", uses: [use("Random", "use", "same")], ready: null, has: null },
    ];
    expect(touchText(touchLines(rows, [{ name: "runs", now: "not on a clock", then: "every 1m" }]))).toEqual([
      "What starts it",
      "  · requests: receive",
      "Folders, databases and secrets",
      "  ! folder inbox: read, +write — the new use approved by confirming; not bound on this server",
      "  · secret api_key: read — set, v1 (a value kept here)",
      "  ! secret other: read — new; not set: the steps that read it get nothing until it is",
      "What it reaches outside",
      "  - host a.example: call — no longer",
      "  + host b.example: call — approved by confirming; done 2 times by the runs since, not undone",
      "What it keeps",
      "  · key count: write — putting the state back changes it",
      "Also",
      "  · randomness: use",
      "Its settings",
      "  ~ runs: not on a clock → every 1m",
    ]);
  });

  it("says nothing for a server that doesn't answer them", () => {
    expect(touchLines(undefined, undefined)).toEqual([]);
  });
});
