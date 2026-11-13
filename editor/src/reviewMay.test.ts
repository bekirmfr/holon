// The Review page's left column (reviewMay.ts): what a version may do, grouped and anchored.

import { describe, expect, it } from "vitest";
import { anchorOf, mayGroups, policyRows, textLines, whileItRuns } from "./reviewMay";
import type { Touch } from "./touches";

const TEXT = `import { flow, route, secret } from "holon"
// It refunds through https://api.stripe.com, with the secret stripe_key.

flow("refunds")

const key = secret("stripe_key")
const shop = database("shop")
route.post("/refund/:id", async (req) => {
  return fetch("https://api.stripe.com/v1/refunds")
})
`;
const lines = textLines(TEXT);

const touch = (kind: string, name: string, change: Touch["change"], approval: boolean, group = "outside"): Touch => ({
  group,
  kind,
  name,
  change,
  uses: [{ item: `${kind}(${name})`, what: kind === "requests" ? "" : "read", words: "", change, approval, optional: false }],
  ready: null,
  has: null,
});

describe("the review's what it may do", () => {
  it("points at the line a thing comes from: its name, quoted first", () => {
    expect(textLines("a\nb\n")).toEqual(["a", "b"]);
    expect(anchorOf({ kind: "secret", name: "stripe_key" }, lines), "past the comment naming it").toBe(6);
    expect(anchorOf({ kind: "host", name: "api.stripe.com" }, lines)).toBe(9);
    expect(anchorOf({ kind: "database", name: "shop/refunds" }, lines), "a part of its name").toBe(7);
    expect(anchorOf({ kind: "requests", name: "" }, lines), "its first route").toBe(8);
    expect(anchorOf({ kind: "clock", name: "" }, lines)).toBeNull();
  });

  it("groups it as a person decides: to approve, unchanged, no longer", () => {
    const groups = mayGroups(
      [touch("host", "api.stripe.com", "new", true), touch("requests", "", "same", false, "triggers"), touch("secret", "old_key", "gone", false, "resources")],
      [{ name: "every", now: "1m", then: "5m" }],
      lines,
    );
    expect(groups.map((g) => [g.key, g.rows.map((r) => [r.line.mark, r.line.text, r.anchor])])).toEqual([
      [
        "approve",
        [
          ["approve", "host api.stripe.com: read", 9],
          ["changed", "every: 1m → 5m", null],
        ],
      ],
      ["same", [["same", "requests", 8]]],
      ["gone", [["gone", "secret old_key: read", null]]],
    ]);
  });
});

describe("the review's while it runs and host policy", () => {
  const defaults = { run_budget_ms: 30000, clock_run_budget_ms: 60000, keep_runs: 1000, keep_run_days: 30 };
  it("says its port, run budget, the runs it keeps, its state and its claims", () => {
    const facts = whileItRuns({
      port: "8301",
      touches: [touch("requests", "", "same", false, "triggers"), touch("key", "count", "new", false, "state"), touch("host", "api.stripe.com", "new", true)],
      limits: { keep_runs: 50 },
      defaults,
    });
    expect(Object.fromEntries(facts)).toEqual({
      port: ":8301",
      "run budget": "30 s a run",
      "runs kept": "the newest 50 (its own; the host's is 1000), none older than 30 days",
      "its state": "key count",
      "its claims": "what a run read before it calls out, held by that run until it commits: one run at a time calls out over the same key",
    });
    const alone = Object.fromEntries(whileItRuns({ port: null, touches: [touch("clock", "", "same", false, "other")], defaults }));
    expect([alone.port, alone["run budget"], alone["its state"]]).toEqual(["none: nothing calls it over HTTP", "60 s a clock run", "nothing: each run starts afresh"]);
  });

  it("checks each line of the host's policy it relies on", () => {
    const policy = { defaults: { max_requests: 10 }, ceilings: { max_requests: 50 }, auto: ["StateWrite(*)"], allow_plain_http: false, allow_private_addresses: false, agents_propose: true };
    const host: Touch = { ...touch("host", "api.stripe.com", "new", true), uses: [{ item: 'HttpRequest("api.stripe.com")', what: "call", words: "", change: "new", approval: true, optional: false }] };
    const rows = policyRows({ touches: [host], items: ['HttpRequest("api.stripe.com")', 'StateWrite("count")'], limits: { max_requests: 20 }, policy, agent: true, may: true, approving: true });
    expect(rows).toEqual([
      { ok: true, text: "it calls api.stripe.com: the host allows it" },
      { ok: true, text: "max_requests = 20: above the host's 10, within its ceiling of 50" },
      { ok: true, text: 'StateWrite("count"): the host grants it without asking (auto StateWrite(*))' },
      { ok: true, text: "agents may propose versions (agents.propose)" },
      { ok: true, text: "you may approve what it asks for (flow.approve-deploy)" },
    ]);
    expect(policyRows({ policy: null, agent: false, may: "maria doesn't approve deploys of refunds", approving: false })).toEqual([{ ok: false, text: "maria doesn't approve deploys of refunds" }]);
  });
});
