// Who holds what, as the People pages say it (access.ts).
import { describe, expect, it } from "vitest";
import { bindingSaid, hostRoleOf, otherBindings, rolesFor, roleTitle } from "./access";
import type { HeldBinding, Principal, RoleEntry } from "./server";

const bound = (role: string, more: Partial<HeldBinding> = {}): HeldBinding => ({ id: 0, role, allows: [], scope: "host", group: null, by: "ada", at: 0, expires_at: null, expired: false, ...more });
const principal = (bindings: HeldBinding[]): Principal => ({
  name: "bob",
  kind: "person",
  role: null,
  bindings,
  display: null,
  email: null,
  disabled: false,
  created_at: 0,
  created_by: null,
  owner: null,
  owner_disabled: false,
  invited: false,
});
const role = (name: string, kinds: RoleEntry["kinds"], builtin: boolean, about: string | null, allows: string[] = []): RoleEntry => ({ name, about, allows, kinds, builtin, bound: 0 });

describe("what a principal holds", () => {
  it("says each binding where it holds, through which group, until when", () => {
    expect(bindingSaid(bound("viewer"))).toBe("viewer");
    expect(bindingSaid(bound("editor", { scope: { flows: { slug: "payout" } }, group: "payments" }))).toBe("editor on payout via payments");
    expect(bindingSaid(bound("approver", { scope: { approval: { flow: { slug: "payout" }, name: "ok" } }, expires_at: 1_796_083_200 + 32_400, expired: true }))).toBe(
      "approver at payout:ok until 2026-12-01 09:00 UTC (expired)",
    );
  });
  it("is set on the host by its own binding there, not a group's, a flow's or an expired one", () => {
    const own = bound("payout-approver");
    const others = [bound("editor", { group: "ops" }), bound("editor", { scope: { flows: { slug: "payout" } } }), bound("admin", { expired: true })];
    const p = principal([...others, own]);
    expect(hostRoleOf(p)).toBe("payout-approver");
    expect(otherBindings(p)).toEqual(others);
    expect(hostRoleOf(principal(others))).toBe("");
    expect(otherBindings(principal(others))).toEqual(others);
  });
});

describe("the roles offered", () => {
  const roles = [
    role("viewer", ["person", "agent", "service"], true, "reads"),
    role("agent", ["agent"], true, "proposes"),
    role("payout-approver", ["person"], false, "decides payouts", ["run.view", "run.decide"]),
    role("auditor", ["person", "service"], false, null, ["run.view", "run.export"]),
  ];
  it("are those the kind may hold, the ones an admin defined included", () => {
    expect(rolesFor(roles, "person").map((r) => r.name)).toEqual(["viewer", "payout-approver", "auditor"]);
    expect(rolesFor(roles, "agent").map((r) => r.name)).toEqual(["viewer", "agent"]);
    expect(rolesFor(roles, "service").map((r) => r.name)).toEqual(["viewer", "auditor"]);
  });
  it("each say what they are for, a defined one what it allows", () => {
    expect(roleTitle(roles, "viewer")).toBe("reads");
    expect(roleTitle(roles, "payout-approver")).toBe("decides payouts: allows run.view, run.decide");
    expect(roleTitle(roles, "auditor")).toBe("allows run.view, run.export");
    expect(roleTitle(roles, "nope")).toBe("");
  });
});
