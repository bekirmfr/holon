// Who holds what, as the People pages say it (access.ts): each binding in a few words, where it
// holds and until when, what a principal's bindings give as `can` decides, and what a version of
// the access document changed. And an access change that waits for people, as Approvals says it:
// where it stands, what deciding it did, and whether it is plainly not yours to approve.
import { describe, expect, it } from "vitest";
import {
  allowsSaid,
  approvalsOf,
  bindingSaid,
  catalogueOf,
  changesOf,
  changesToDecide,
  changeState,
  decidedChange,
  flowOf,
  givesOf,
  heldBy,
  hostRoleOf,
  notYours,
  otherBindings,
  rolesFor,
  roleTitle,
  scopeSaid,
  seesFlow,
  subjectSaid,
  untilOf,
  waitingOf,
  waitsSaid,
  type Tagged,
} from "./access";
import type { AccessDocument, AccessProposal, BindingEntry, HeldBinding, Kind, Principal, RoleEntry, Scope, Subject } from "./server";

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
    expect(bindingSaid(bound("editor", { scope: { flows: { tag: "payments" } } }))).toBe("editor on tag payments");
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

// ── where, until, what bindings give, a version's changes ─────────────────────────────────────

/** The catalogue, in its order (can.rs `Action::ALL`). */
const CATALOGUE = [
  "flow.view", "flow.edit", "flow.propose", "flow.deploy", "flow.approve-deploy", "flow.restore", "flow.restore-dry-run", "flow.lifecycle", "flow.delete",
  "run.view", "run.call", "run.decide", "run.export",
  "state.view", "state.write",
  "secret.list", "secret.set", "secret.roll-back",
  "resource.view", "resource.bind", "resource.unbind",
  "people.view", "people.manage", "people.assign",
  "role.view", "role.define",
  "policy.view", "policy.edit",
  "host.view", "host.check", "host.operate",
];
const VIEWER = ["flow.view", "run.view", "run.export", "state.view", "secret.list", "policy.view"];
const AGENT = ["flow.view", "flow.edit", "flow.propose", "flow.restore-dry-run", "run.view", "run.call", "run.export", "state.view", "secret.list", "policy.view"];
const EDITOR = [
  "flow.view", "flow.edit", "flow.propose", "flow.deploy", "flow.approve-deploy", "flow.restore", "flow.restore-dry-run", "flow.lifecycle", "flow.delete",
  "run.view", "run.call", "run.decide", "run.export", "state.view", "state.write", "secret.list", "policy.view", "policy.edit", "host.check",
];
/** The roles as `GET /roles` lists them: the built-in ones, and two an admin defined. */
const ROLES: RoleEntry[] = [
  role("viewer", ["person", "agent", "service"], true, "reads", VIEWER),
  role("agent", ["agent"], true, "proposes", AGENT),
  role("editor", ["person", "service"], true, "operates", EDITOR),
  role("admin", ["person"], true, "everything", CATALOGUE),
  role("approver", ["person", "agent", "service"], false, null, ["run.decide"]),
  role("operator", ["agent", "person"], false, null, ["flow.deploy", "flow.lifecycle", "people.view", "host.view"]),
];
const FLOWS: Tagged[] = [
  { slug: "payout", tags: ["payments"] },
  { slug: "refunds", tags: ["payments", "risk"] },
  { slug: "hello", tags: [] },
];
const at = (slug: string, name: string): Scope => ({ approval: { flow: { slug }, name } });
const flowScope = (slug: string): Scope => ({ flows: { slug } });
const tagScope = (tag: string): Scope => ({ flows: { tag } });
/** `role` bound at `scope`, with what the role allows, as `GET /people` lists it. */
const held = (name: string, scope: Scope, more: Partial<HeldBinding> = {}) => bound(name, { scope, allows: ROLES.find((r) => r.name === name)?.allows ?? [], ...more });
const FLOW_PART = (allows: string[]) => allows.filter((a) => ["flow", "run", "state", "secret"].includes(a.split(".")[0]));
const HOST_PART = (allows: string[]) => allows.filter((a) => !["flow", "run", "state", "secret"].includes(a.split(".")[0]));

describe("where a binding holds and until when", () => {
  it("says a scope and a subject in the server's words", () => {
    expect(scopeSaid("host")).toBe("on this host");
    expect(scopeSaid(flowScope("payout"))).toBe('on "payout"');
    expect(scopeSaid(tagScope("payments"))).toBe('on the flows tagged "payments"');
    expect(scopeSaid(at("payout", "ok"))).toBe(`for "payout"'s approval "ok"`);
    expect(subjectSaid({ principal: "bob" })).toBe("bob");
    expect(subjectSaid({ group: "payments" })).toBe("the group payments");
    expect([flowOf("host"), flowOf(flowScope("payout")), flowOf(tagScope("payments")), flowOf(at("refunds", "ok"))]).toEqual([null, "payout", null, "refunds"]);
  });
  it("reads until as holon bind --until does: a while, or a moment in UTC", () => {
    const now = 1_800_000_000;
    // The CLI's own table (accesscmd.rs until_reads_a_date_in_utc_or_a_while).
    const table: [string, number][] = [
      ["30m", now + 1_800],
      ["12h", now + 43_200],
      ["7d", now + 604_800],
      ["2w", now + 1_209_600],
      ["1970-01-01", 0],
      ["2026-12-01", 1_796_083_200],
      ["2026-12-01T09:00", 1_796_083_200 + 32_400],
      ["2000-02-29T23:59:59", 951_868_799],
      [" 7d ", now + 604_800],
    ];
    for (const [text, want] of table) expect(untilOf(text, now), text).toEqual({ at: want });
    expect(untilOf("", now)).toBeNull();
    expect(untilOf("   ", now)).toBeNull();
    for (const wrong of ["7", "7y", "d", "2026-13-01", "2026-12-01T25:00", "2026-12", "2026-12-01T09", "1969-12-31", "soon", "2026-12-01t09:00", "7D"]) {
      expect(untilOf(wrong, now), wrong).toEqual({ error: `until is a date in UTC (2026-12-01, 2026-12-01T09:00) or a while (30m, 12h, 7d, 2w), not ${JSON.stringify(wrong)}` });
    }
  });
});

describe("what a principal's bindings give", () => {
  const none = { host: [], everyFlow: [], own: [], tags: [], flows: [] };
  it("is, for each way of being bound, what can decides", () => {
    const table: [string, Kind, HeldBinding[], ReturnType<typeof givesOf>][] = [
      [
        "an admin on the host holds the host's own and every flow's, those deployed later too",
        "person",
        [held("admin", "host")],
        {
          host: HOST_PART(CATALOGUE),
          everyFlow: FLOW_PART(CATALOGUE),
          own: [],
          tags: [],
          flows: FLOWS.map((f) => ({ slug: f.slug, actions: FLOW_PART(CATALOGUE), decides: [] })).sort((a, b) => a.slug.localeCompare(b.slug)),
        },
      ],
      [
        "a viewer on one flow: that flow's part, not the policy, which is the host's",
        "person",
        [held("viewer", flowScope("payout"))],
        { ...none, flows: [{ slug: "payout", actions: FLOW_PART(VIEWER), decides: [] }] },
      ],
      [
        "an editor on a tag: each flow deployed with it, and the tag for those to come",
        "person",
        [held("editor", tagScope("payments"))],
        {
          ...none,
          tags: [{ tag: "payments", actions: FLOW_PART(EDITOR) }],
          flows: [
            { slug: "payout", actions: FLOW_PART(EDITOR), decides: [] },
            { slug: "refunds", actions: FLOW_PART(EDITOR), decides: [] },
          ],
        },
      ],
      [
        "a decider at one approval point: deciding it, and nothing else of the flow",
        "person",
        [held("approver", at("payout", "ok")), held("approver", at("payout", "big"))],
        { ...none, flows: [{ slug: "payout", actions: [], decides: ["big", "ok"] }] },
      ],
      [
        "bindings add up: a viewer on the host and an editor on one flow",
        "person",
        [held("editor", flowScope("hello")), held("viewer", "host")],
        {
          ...none,
          host: ["policy.view"],
          everyFlow: FLOW_PART(VIEWER),
          flows: [
            { slug: "hello", actions: FLOW_PART(EDITOR), decides: [] },
            { slug: "payout", actions: FLOW_PART(VIEWER), decides: [] },
            { slug: "refunds", actions: FLOW_PART(VIEWER), decides: [] },
          ],
        },
      ],
      [
        "a binding on a flow not deployed yet holds there already",
        "person",
        [held("viewer", flowScope("later"))],
        { ...none, flows: [{ slug: "later", actions: FLOW_PART(VIEWER), decides: [] }] },
      ],
      ["an expired binding gives nothing", "person", [held("admin", "host", { expired: true })], none],
      ["a role its kind may not hold gives nothing: an editor bound to an agent", "agent", [held("editor", "host")], none],
      [
        "the agent role saves only its own flows",
        "agent",
        [held("agent", "host")],
        {
          ...none,
          host: ["policy.view"],
          everyFlow: FLOW_PART(AGENT).filter((a) => a !== "flow.edit"),
          own: ["flow.edit"],
          flows: FLOWS.map((f) => ({ slug: f.slug, actions: FLOW_PART(AGENT).filter((a) => a !== "flow.edit"), decides: [] })).sort((a, b) => a.slug.localeCompare(b.slug)),
        },
      ],
      [
        "no role gives an agent what is a person's: an admin's role allowing it to start flows and read people",
        "agent",
        [held("operator", "host")],
        {
          ...none,
          host: ["host.view"],
          everyFlow: ["flow.deploy"],
          flows: FLOWS.map((f) => ({ slug: f.slug, actions: ["flow.deploy"], decides: [] })).sort((a, b) => a.slug.localeCompare(b.slug)),
        },
      ],
      ["a service decides no approval", "service", [held("approver", at("payout", "ok"))], none],
      [
        "an editor on the host decides every approval: no point is named",
        "person",
        [held("editor", "host"), held("approver", at("payout", "ok"))],
        {
          ...none,
          host: ["policy.view", "policy.edit", "host.check"],
          everyFlow: FLOW_PART(EDITOR),
          flows: FLOWS.map((f) => ({ slug: f.slug, actions: FLOW_PART(EDITOR), decides: f.slug === "payout" ? ["ok"] : [] })).sort((a, b) => a.slug.localeCompare(b.slug)),
        },
      ],
    ];
    for (const [what, kind, bindings, want] of table) expect(givesOf(kind, bindings, ROLES, FLOWS), what).toEqual(want);
  });
  it("lists each action in the catalogue's order, however the bindings list them", () => {
    const shuffled = bound("custom", { scope: "host", allows: ["state.view", "run.view", "flow.view"] });
    const roles = [...ROLES, role("custom", ["person"], false, null, ["state.view", "run.view", "flow.view"])];
    expect(givesOf("person", [shuffled], roles, []).everyFlow).toEqual(["flow.view", "run.view", "state.view"]);
  });
  it("says whether whoever is bound sees the flow, as holon bind asks it", () => {
    const people = [principal([held("approver", at("payout", "ok"))]), { ...principal([held("viewer", tagScope("payments")), held("approver", at("payout", "ok"))]), name: "cy" }];
    const groupBound = (role: string, scope: Scope, group = "ops"): BindingEntry => ({ id: 1, who: { group }, role, scope, by: "ada", at: 0, expired: false });
    const table: [string, Subject, BindingEntry[], boolean][] = [
      ["bound only at the approval point: not", { principal: "bob" }, [], false],
      ["a viewer on the flow's tag: yes", { principal: "cy" }, [], true],
      ["someone not listed: it can't tell, and says nothing", { principal: "nobody" }, [], true],
      ["a group bound viewer on the flow: yes", { group: "ops" }, [groupBound("viewer", flowScope("payout"))], true],
      ["a group bound viewer on the host: yes", { group: "ops" }, [groupBound("viewer", "host")], true],
      ["a group bound viewer on another flow: not", { group: "ops" }, [groupBound("viewer", flowScope("hello"))], false],
      ["another group's binding: not", { group: "ops" }, [groupBound("viewer", "host", "risk")], false],
      ["a group bound a role that doesn't view: not", { group: "ops" }, [groupBound("approver", "host")], false],
      ["an expired one: not", { group: "ops" }, [{ ...groupBound("viewer", "host"), expired: true }], false],
    ];
    for (const [what, who, bindings, want] of table) expect(seesFlow(who, "payout", people, bindings, ROLES, FLOWS), what).toBe(want);
  });
  it("reads a deployed version's approval points from its manifest", () => {
    expect(approvalsOf(['Action("pay")', 'Approval("payout/classifier:agent, reviewer, approver")', 'Approval("big/2 people")', 'Approval("payout/1 person")', "StateWrite(\"x\")"])).toEqual(["payout", "big"]);
    expect(approvalsOf([])).toEqual([]);
  });
});

describe("the catalogue and roles", () => {
  it("lists the catalogue by what each action acts on, and says what a role allows", () => {
    expect(catalogueOf(ROLES).map((r) => [r.of, r.actions.length])).toEqual([
      ["flow", 9],
      ["run", 4],
      ["state", 2],
      ["secret", 3],
      ["resource", 3],
      ["people", 3],
      ["role", 2],
      ["policy", 2],
      ["host", 3],
    ]);
    expect(allowsSaid(CATALOGUE, ROLES)).toBe("everything");
    expect(allowsSaid(VIEWER, ROLES)).toBe("flow view · run view, export · state view · secret list · policy view");
    expect(allowsSaid(["run.decide", "flow.view", "run.view"], ROLES)).toBe("flow view · run view, decide");
    expect(allowsSaid([], ROLES)).toBe("");
  });
  it("says who holds a role and where, by the bindings still holding", () => {
    const b = (id: number, who: Subject, role: string, scope: Scope, expired = false): BindingEntry => ({ id, who, role, scope, by: "ada", at: 0, expired });
    const bindings = [b(1, { principal: "bob" }, "approver", "host"), b(2, { group: "ops" }, "approver", flowScope("payout")), b(3, { principal: "cy" }, "approver", "host", true), b(4, { principal: "dee" }, "viewer", "host")];
    expect(heldBy(bindings, "approver")).toBe("bob, the group ops on payout");
    expect(heldBy(bindings, "editor")).toBe("");
  });
});

describe("what a version of the access document changed", () => {
  const doc = (version: number, more: Partial<AccessDocument> = {}): AccessDocument => ({
    version,
    at: 1_796_083_200,
    by: "ada",
    what: "",
    roles: [],
    groups: [],
    bindings: [],
    approvals: { exclude_deployer: true, agents: true, starter: true, changes: null },
    ...more,
  });
  const b = (id: number, role: string, who: Subject, scope: Scope, expires_at?: number) => ({ id, who, role, scope, by: "ada", at: 0, ...(expires_at ? { expires_at } : {}) });
  it("says each binding, role, group and setting added, removed or changed", () => {
    const before = doc(4, {
      roles: [
        { name: "approver", allows: ["run.decide"], kinds: ["person"] },
        { name: "gone", allows: ["run.view"], kinds: ["person"] },
      ],
      groups: [
        { name: "ops", members: ["bob", "cy"] },
        { name: "old", members: [] },
      ],
      bindings: [b(1, "admin", { principal: "ada" }, "host"), b(2, "viewer", { principal: "bob" }, "host")],
      local: "ada",
    });
    const after = doc(5, {
      roles: [
        { name: "approver", about: "decides payouts", allows: ["run.decide", "run.view"], kinds: ["person", "agent"] },
        { name: "auditor", allows: ["run.view", "run.export"], kinds: ["person"] },
      ],
      groups: [
        { name: "ops", members: ["cy", "dee"] },
        { name: "risk", members: [] },
      ],
      bindings: [b(1, "admin", { principal: "ada" }, "host"), b(3, "approver", { group: "ops" }, at("payout", "ok"), 1_796_083_200 + 32_400)],
      approvals: { exclude_deployer: false, agents: true, starter: false, changes: 2 },
      local: "bob",
    });
    expect(changesOf(before, after)).toEqual([
      { of: "binding", change: "removed", said: "viewer to bob on this host" },
      { of: "binding", change: "added", said: `approver to the group ops for "payout"'s approval "ok", until 2026-12-01 09:00 UTC` },
      { of: "role", change: "removed", said: "gone" },
      { of: "role", change: "changed", said: "approver: allows + run.view; held by + agent; about changed" },
      { of: "role", change: "added", said: "auditor: allows run.view, run.export; held by person" },
      { of: "group", change: "removed", said: "old" },
      { of: "group", change: "changed", said: "ops: + dee, − bob" },
      { of: "group", change: "added", said: "risk: no one" },
      { of: "approvals", change: "changed", said: "exclude_deployer off" },
      { of: "approvals", change: "changed", said: "starter off" },
      { of: "approvals", change: "changed", said: "changes 2" },
      { of: "run file", change: "changed", said: "the run file signs in as bob (was ada)" },
    ]);
    expect(changesOf(after, after)).toEqual([]);
  });
  it("lists everything of the first version as added, and no setting", () => {
    const first = doc(1, { bindings: [b(1, "admin", { principal: "ada" }, "host")], local: "ada", approvals: { exclude_deployer: false, agents: false, starter: true, changes: null } });
    expect(changesOf(null, first)).toEqual([
      { of: "binding", change: "added", said: "admin to ada on this host" },
      { of: "run file", change: "added", said: "the run file signs in as ada" },
    ]);
  });
});

describe("an access change that waits for people", () => {
  const change = (id: number, more: Partial<AccessProposal>): AccessProposal => ({
    id,
    author: "ada",
    at: 60,
    what: `bound approver to dee on "pay" (${id})`,
    gives: ['run.decide, run.view to dee on "pay", as approver'],
    to: ["dee"],
    base: 3,
    needs: 2,
    approvals: [],
    state: "waiting",
    ...more,
  });
  const waits = change(5, { approvals: [{ who: "bob", at: 61 }], ways_in: ["cy made dee's invite link"] });
  const fresh = change(4, {});
  const one = change(6, { needs: 1 });
  const made = change(3, { state: "applied", version: 12, approvals: [{ who: "bob", at: 61 }, { who: "cy", at: 62 }] });
  const declined = change(2, { state: "declined", declined_by: "bob", reason: "not this week" });
  const withdrawn = change(1, { state: "declined", declined_by: "ada", reason: "the wrong flow" });
  const stale = change(0, { state: "stale", stale: 'no role "approver"' });

  it("says where each stands, and what deciding it did, as holon access proposals and approve do (accesscmd.rs)", () => {
    const table: [AccessProposal, string, string][] = [
      [waits, "waits, 1 of 2 approved (bob)", `Approved access change 5: it waits for 1 more person (bound approver to dee on "pay" (5)).`],
      [fresh, "waits, 0 of 2 approved", `Approved access change 4: it waits for 2 more people (bound approver to dee on "pay" (4)).`],
      [made, "made as version 12 (approved by bob, cy)", `Approved access change 3, the last approval it waited for: made, as access version 12: bound approver to dee on "pay" (3).`],
      [declined, "declined by bob: not this week", `Declined access change 2: bound approver to dee on "pay" (2).`],
      [withdrawn, "withdrawn by ada: the wrong flow", `Withdrew access change 1: bound approver to dee on "pay" (1).`],
      [stale, 'stale: no role "approver"', `Access change 0 no longer applies: no role "approver".`],
    ];
    for (const [p, state, decided] of table) {
      expect(changeState(p), `${p.id}`).toBe(state);
      expect(decidedChange(p), `${p.id}`).toBe(decided);
    }
  });

  it("says a change kept as one, as holon bind does, and nothing of a change made", () => {
    expect(waitsSaid(fresh)).toBe(`bound approver to dee on "pay" (4): waits for 2 people other than you to approve it (access change 4)`);
    expect(waitsSaid(one)).toBe(`bound approver to dee on "pay" (6): waits for 1 person other than you to approve it (access change 6)`);
    expect(waitingOf({ proposal: one, version: 9 })).toBe(waitsSaid(one));
    for (const made of [{ binding: {}, version: 9 }, { name: "dee", role: "viewer" }, undefined, null, "done"]) expect(waitingOf(made)).toBeNull();
  });

  it("says why someone plainly doesn't approve it, as the server would refuse them; else leaves it to the server", () => {
    const table: [string, string | null][] = [
      ["ada", "you proposed it: people other than you approve it"],
      ["bob", "you approved it"],
      ["cy", "it gives more to someone you could act as (cy made dee's invite link): it is approved by someone who couldn't"],
      ["c", null],
      ["eve", null],
    ];
    for (const [me, said] of table) expect(notYours(waits, me), me).toBe(said);
    // Each that waits and isn't plainly not theirs is theirs to decide; the decided never are.
    expect(changesToDecide([waits, fresh, one, made, declined, withdrawn, stale], "eve")).toBe(3);
    expect(changesToDecide([waits, fresh, one, made], "bob")).toBe(2);
    expect(changesToDecide([waits, fresh, one], "ada")).toBe(0);
  });
});
