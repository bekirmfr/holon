// An approval a run waits at, as the editor says it and who may decide it (decisions.ts).
import { describe, expect, it } from "vitest";
import { approvalSaid, decisionSaid, deciderOf, mayDecide, pointKey, toDecide, type WaitingPoint } from "./decisions";
import type { WValue } from "./holon";
import type { WaitingRun } from "./server";

const took = (who: string, approved = true) => ({ slot: 0, who, kind: "person" as const, at: 0, approved });
const point = (people: number, ...by: string[]): WaitingPoint => ({ name: "payout", expires_at: 0, people, decisions: by.map((b) => took(b)) });
/** One a decline a run lock turned away is kept on (durable.rs): it counts no approval. */
const declined = (people: number, ...by: string[]): WaitingPoint => ({ ...point(people, ...by), decisions: [took("bob", false), ...by.map((b) => took(b))] });
const ada = { name: "ada", kind: "person" as const, role: "editor" as const };
const t = (value: string): WValue => ({ type: "Text", value });
const record = (fields: [string, WValue][]): WValue => ({ type: "Record", value: fields });

describe("who decides an approval", () => {
  it("decides by their name, the local admin by the name it runs as", () => {
    expect(deciderOf(ada)).toBe("ada");
    expect(deciderOf({ name: "local:ada", kind: "person", role: "admin" })).toBe("ada");
  });
  it("decides each approval once, where the server says they may, and never an event", () => {
    expect(mayDecide(point(2), ada, true)).toBe("decide");
    expect(mayDecide(point(2, "ada"), ada, true)).toBe("approved");
    expect(mayDecide(point(2, "bob"), { name: "local:ada", kind: "person", role: "admin" }, true)).toBe("decide");
    expect(mayDecide(declined(2, "ada"), ada, true)).toBe("approved");
    expect(mayDecide(declined(2), { name: "bob", kind: "person", role: "editor" }, true)).toBe("decide");
    expect(mayDecide({ name: "paid", expires_at: 0 }, ada, true)).toBe("no");
    // Whatever their role on the host: a viewer bound at the point decides, an editor not bound there doesn't.
    expect(mayDecide(point(1), { name: "cy", kind: "person", role: "viewer" }, true)).toBe("decide");
    expect(mayDecide(point(1), ada, false)).toBe("no");
  });
  it("counts what is still theirs to decide, point by point", () => {
    const run = (slug: string, ...waiting: WaitingPoint[]): { slug: string; run: WaitingRun } => ({ slug, run: { run_id: "r", kind: "trigger", route: "pay", parked_at: 0, waiting, delivered: [] } });
    const decides = new Map([
      [pointKey("payout", "payout"), true],
      [pointKey("refunds", "payout"), false],
    ]);
    const runs = [run("payout", point(2)), run("payout", point(2, "ada")), run("payout", { name: "paid", expires_at: 0 }), run("payout", point(1, "bob"), point(3)), run("refunds", point(1))];
    expect(toDecide(runs, ada, decides)).toBe(3);
    expect(toDecide(runs, ada, new Map())).toBe(0);
  });
});

describe("an approval, said", () => {
  it("says how many people, and who approved so far", () => {
    expect(approvalSaid(point(1))).toBe(`a person's approval "payout"`);
    expect(approvalSaid(point(2))).toBe(`2 people's approval "payout"`);
    expect(approvalSaid(point(3, "ada", "bob"))).toBe(`3 people's approval "payout" · 2 of 3: ada and bob approved`);
    expect(approvalSaid(declined(2, "ada"))).toBe(`2 people's approval "payout" · 1 of 2: ada approved`);
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
