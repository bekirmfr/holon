import { describe, expect, it } from "vitest";
import { askAgain, cannotDecide, touchLines, touchText, type Touch, type TouchUse } from "./touches";

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
      { group: "outside", kind: "host", name: "c.example", change: "same", uses: [use('HttpRequest("c.example")', "call", "same", true)], ready: null, has: null },
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
      "  + host c.example: call — asked again: approved by confirming",
      "What it keeps",
      "  · key count: write — putting the state back changes it",
      "Also",
      "  · randomness: use",
      "Its settings",
      "  ~ runs: not on a clock → every 1m",
    ]);
  });

  // The rows of touches.rs's a_tag_s_row_says_who_it_lets_in, said as it says them.
  it("says who a tag lets act on the flow: joined, left or kept", () => {
    const tag = (name: string, who: string, change: TouchUse["change"]): Touch => ({
      group: "access",
      kind: "tag",
      name,
      change,
      uses: [use(`Tag("${name}")`, who, change, change === "new")],
      ready: null,
      has: null,
    });
    const rows = [tag("old", "the group ops as viewer", "gone"), tag("ops", "no one is bound on it yet", "same"), tag("payments", "bob as editor", "new")];
    expect(touchText(touchLines(rows, []))).toEqual([
      "Who may act on it",
      "  - leaves old: the group ops as viewer — no longer",
      "  · tagged ops: no one is bound on it yet",
      "  + joins payments: bob as editor — approved by confirming",
    ]);
  });

  // Approvals' rows as a deploy's preview answers them (touches.rs `part`, deploy.rs
  // `approvals_unready`, as tests/deploy_review.rs words them): new, as now, gone, and changed
  // (before and after), two of them with a step no one here may take, and one only the runs ada
  // starts would wait at, as she alone approves.
  it("says each approval's steps, and which no one here can decide; deploying asks again, naming each no run of could be", () => {
    const approval = (name: string, uses: TouchUse[], change: Touch["change"], ready: string | null, waits: Touch["waits"] = null): Touch => ({ group: "triggers", kind: "approval", name, change, uses, ready, waits, has: null });
    const starts = "a run ada starts can't be decided, as they take no step of it: approver: no person holds it here";
    const rows: Touch[] = [
      approval("payout", [use('Approval("payout/agent:classifier>person:approver")', "approved by an agent holding classifier, then a person holding approver", "new", true)], "new", "classifier: no agent holds it here", "every run"),
      approval("refund", [use('Approval("refund/person")', "approved by a person", "same")], "same", null),
      approval("old", [use('Approval("old/person")', "approved by a person", "gone")], "gone", null),
      approval(
        "review",
        [use('Approval("review/person:reviewer>person")', "approved by a person holding reviewer, then a person", "new", true), use('Approval("review/person")', "approved by a person", "gone")],
        "changed",
        "reviewer: no person holds it here",
        "every run",
      ),
      approval("settle", [use('Approval("settle/person:approver")', "approved by a person holding approver", "new", true)], "new", starts, "some runs"),
      // Not ready, but not an approval: deploying doesn't ask again for it.
      { group: "resources", kind: "secret", name: "key", change: "new", uses: [use('SecretRead("key")', "read", "new")], ready: "not set: the steps that read it get nothing until it is", has: null },
    ];
    expect(touchText(touchLines(rows))).toEqual([
      "What starts it",
      "  ! approval payout: approved by an agent holding classifier, then a person holding approver — approved by confirming; classifier: no agent holds it here",
      "  · approval refund: approved by a person",
      "  - approval old: approved by a person — no longer",
      "  ! approval review: +approved by a person holding reviewer, then a person, −approved by a person — the new use approved by confirming; reviewer: no person holds it here",
      `  ! approval settle: approved by a person holding approver — approved by confirming; ${starts}`,
      "Folders, databases and secrets",
      "  ! secret key: read — new; not set: the steps that read it get nothing until it is",
    ]);
    expect(rows.filter(cannotDecide).map((t) => t.name)).toEqual(["payout", "review", "settle"]);
    expect(askAgain("payouts", rows, "Approve and deploy")).toBe(
      [
        'Runs of "payouts" would wait at approvals no one here can decide yet:',
        "",
        "• payout: classifier: no agent holds it here",
        "• review: reviewer: no person holds it here",
        "",
        "They wait there until someone here may take each step (People and access, or holon bind, binds them). Approve and deploy it anyway?",
      ].join("\n"),
    );
    expect(askAgain("payouts", rows.slice(0, 1), "Propose")).toMatch(/^Runs of "payouts" would wait at an approval no one here can decide yet:\n\n• payout: classifier: no agent holds it here\n\n.* Propose it anyway\?$/);
    // Nothing to ask: every approval can be decided, or it has none, or only the runs ada starts
    // would wait (settle: those a route starts can be decided).
    expect(askAgain("payouts", rows.filter((t) => !cannotDecide(t)), "Deploy")).toBeNull();
    expect(askAgain("payouts", rows.filter((t) => t.name === "settle"), "Deploy")).toBeNull();
    expect(askAgain("payouts", undefined, "Deploy")).toBeNull();
  });

  it("says nothing for a server that doesn't answer them", () => {
    expect(touchLines(undefined, undefined)).toEqual([]);
  });
});
