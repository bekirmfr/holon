// An approval a run waits at, as the editor says it and who may decide it (decisions.ts).
import { describe, expect, it } from "vitest";
import { approvalSaid, decisionSaid, deciderOf, mayDecide, toDecide, type WaitingPoint } from "./decisions";
import type { WValue } from "./holon";
import type { WaitingRun } from "./server";

const point = (people: number, ...by: string[]): WaitingPoint => ({ name: "payout", expires_at: 0, people, approved: by.map((b) => ({ by: b, at: 0 })) });
const ada = { name: "ada", kind: "person" as const, role: "editor" as const };
const t = (value: string): WValue => ({ type: "Text", value });
const record = (fields: [string, WValue][]): WValue => ({ type: "Record", value: fields });

describe("who decides an approval", () => {
  it("is a person, an editor or an admin, the local admin by its name", () => {
    expect(deciderOf(ada)).toBe("ada");
    expect(deciderOf({ name: "local:ada", kind: "person", role: "admin" })).toBe("ada");
    expect(deciderOf({ name: "cy", kind: "person", role: "viewer" })).toBeNull();
    expect(deciderOf({ name: "ci", kind: "service", role: "editor" })).toBeNull();
    expect(deciderOf({ name: "claude", kind: "agent", role: "agent" })).toBeNull();
    expect(deciderOf({ name: "claude", kind: "agent", role: "viewer" })).toBeNull();
  });
  it("decides each approval once, and never an event", () => {
    expect(mayDecide(point(2), ada)).toBe("decide");
    expect(mayDecide(point(2, "ada"), ada)).toBe("approved");
    expect(mayDecide(point(2, "bob"), { name: "local:ada", kind: "person", role: "admin" })).toBe("decide");
    expect(mayDecide({ name: "paid", expires_at: 0 }, ada)).toBe("no");
    expect(mayDecide(point(1), { name: "cy", kind: "person", role: "viewer" })).toBe("no");
  });
  it("counts what is still theirs to decide", () => {
    const run = (...waiting: WaitingPoint[]): { run: WaitingRun } => ({ run: { run_id: "r", kind: "trigger", route: "pay", parked_at: 0, waiting, delivered: [] } });
    expect(toDecide([run(point(2)), run(point(2, "ada")), run({ name: "paid", expires_at: 0 }), run(point(1, "bob"), point(3))], ada)).toBe(3);
  });
});

describe("an approval, said", () => {
  it("says how many people, and who approved so far", () => {
    expect(approvalSaid(point(1))).toBe(`a person's approval "payout"`);
    expect(approvalSaid(point(2))).toBe(`2 people's approval "payout"`);
    expect(approvalSaid(point(3, "ada", "bob"))).toBe(`3 people's approval "payout" · 2 of 3: ada and bob approved`);
  });
  it("says the decision a run was given", () => {
    const by = (...names: string[]): WValue => ({ type: "List", value: names.map(t) });
    const no: WValue = { type: "Boolean", value: false };
    expect(decisionSaid(record([["approved", { type: "Boolean", value: true }], ["by", by("ada", "bob")]]))).toBe("approved by ada and bob");
    expect(decisionSaid(record([["approved", no], ["by", by()], ["declined_by", t("bob")], ["reason", t("not this vendor")]]))).toBe("declined by bob: not this vendor");
    expect(decisionSaid(record([["approved", no], ["by", by("ada")], ["expired", { type: "Boolean", value: true }]]))).toBe("ran out undecided, approved by ada before");
    expect(decisionSaid(record([["method", t("GET")]]))).toBeNull();
  });
});
