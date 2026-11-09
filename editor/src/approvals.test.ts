// What waits for a person (approvals.ts), derived as the workbench and `holon mcp` derive it.

import { describe, expect, it } from "vitest";
import { approvalsOf, keptKey, pendingAsks, reviewQueue, routeOf } from "./approvals";
import type { DeploymentEntry, DeploymentEvent, FlowListEntry } from "./server";

const deployment = (slug: string, at: number, more: Partial<DeploymentEntry> = {}): DeploymentEntry => ({
  slug,
  unit: `u-${slug}`,
  triggers: [],
  has_state: false,
  trigger_count: 0,
  error_count: 0,
  deployed_at_epoch_seconds: at,
  port: null,
  status: "running",
  session: 1,
  sessions: 1,
  ...more,
});
const saved = (name: string, at: number, by?: string): FlowListEntry => ({ name, saved_at_epoch_seconds: at, ...(by ? { saved_by: by } : {}) });
const event = (seq: number, kind: string, more: Partial<DeploymentEvent> = {}): DeploymentEvent => ({ seq, at_ms: seq * 1000, run_seq: 0, kind, what: `${kind} ${seq}`, ...more });

describe("approvals", () => {
  it("are a version saved since it was deployed, and a saved flow never deployed: deployments first", () => {
    const got = approvalsOf([deployment("refunds", 100), deployment("quiet", 100)], [saved("drafty", 50, "ada"), saved("refunds", 200, "local:codex (agent)"), saved("quiet", 90)], {
      versions: new Map([["refunds", 3]]),
    });
    expect(got).toEqual([
      { kind: "update", slug: "refunds", by: "local:codex (agent)", at: 200, version: 4 },
      { kind: "first", slug: "drafty", by: "ada", at: 50, version: 1 },
    ]);
    expect(reviewQueue(got).map((a) => a.slug)).toEqual(["refunds", "drafty"]);
  });

  it("number a version once its deployment's versions are read", () => {
    const [update] = approvalsOf([deployment("refunds", 100)], [saved("refunds", 200)]);
    expect(update).toMatchObject({ kind: "update", version: null });
    expect(routeOf(update)).toBeNull();
    expect(routeOf({ ...update, version: 2 } as typeof update)).toEqual({ kind: "review", slug: "refunds", version: 2 });
  });

  it("are why a deployment can't run, and a port it can't listen on", () => {
    const got = approvalsOf([deployment("shop", 100, { attention: "its grant no longer covers DbWrite", listen_error: "port 8150 is taken" })], []);
    expect(got.map((a) => a.kind)).toEqual(["attention", "port"]);
    expect(routeOf(got[0])).toEqual({ kind: "flow", slug: "shop", tab: "status" });
  });

  it("leave out a version kept saved, until it is saved again", () => {
    const kept = new Set([keptKey("drafty", 50)]);
    expect(approvalsOf([], [saved("drafty", 50)], { kept })).toEqual([]);
    expect(approvalsOf([], [saved("drafty", 60)], { kept })).toHaveLength(1);
  });

  it("are an agent's asks to restore, until a restore from the same point answers one", () => {
    const events = [event(1, "deploy"), event(2, "ask", { from: "run 4", by: "agent:codex" }), event(3, "ask", { from: "version 1" }), event(4, "restore", { from: "version 1" })];
    expect(pendingAsks(events).map((e) => e.seq)).toEqual([2]);
    const got = approvalsOf([deployment("notes", 100)], [], { asks: new Map([["notes", events]]) });
    expect(got).toEqual([{ kind: "ask", slug: "notes", what: "ask 2", by: "agent:codex", at: 2, from: "run 4" }]);
    expect(routeOf(got[0])).toEqual({ kind: "flow", slug: "notes", tab: "history" });
  });
});
