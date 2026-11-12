// An approval a run waits at, as the editor says it and which steps whoever is signed in takes
// (decisions.ts).
import { describe, expect, it } from "vitest";
import { approvalSaid, decidedSaid, decisionSaid, deciderOf, toDecide, waitsAfter, yourDecision, yourSteps, type WaitingPoint, type WaitingStep } from "./decisions";
import type { WValue } from "./holon";
import type { Decided, WaitingRun } from "./server";

const took = (who: string, slot = 0, role?: string, approved = true) => ({ slot, who, kind: "person" as const, role, at: 0, approved });
const step = (slot: number, people: number, by: string[], open: boolean, role?: string, kind: "person" | "agent" = "person"): WaitingStep => ({ slot, stage: slot, role, kind, people, by, open });
/** By people alone: one step of `people`, approved so far by `by`. */
const alone = (people: number, by: string[] = [], yours: number[] = []): WaitingPoint => ({
  name: "payout",
  expires_at: 0,
  steps: people === 1 ? "person" : `person*${people}`,
  said: people === 1 ? "a person" : `${people} people`,
  slots: [step(0, people, by, by.length < people)],
  decisions: by.map((b) => took(b)),
  yours,
});
/** A chain: an agent classifies, then a person reviews (two of them), then one approves. */
const chain = (classified: string[], reviewed: string[], yours: number[] = []): WaitingPoint => ({
  name: "payout",
  expires_at: 0,
  steps: "agent:classifier>person:reviewer*2>person:approver",
  said: "an agent holding classifier, then 2 people holding reviewer, then a person holding approver",
  slots: [
    step(0, 1, classified, classified.length === 0, "classifier", "agent"),
    step(1, 2, reviewed, classified.length > 0 && reviewed.length < 2, "reviewer"),
    step(2, 1, [], classified.length > 0 && reviewed.length >= 2, "approver"),
  ],
  decisions: [...classified.map((b) => ({ ...took(b, 0, "classifier"), kind: "agent" as const })), ...reviewed.map((b) => took(b, 1, "reviewer"))],
  yours,
});
const ada = { name: "ada", kind: "person" as const, role: "editor" as const };
const t = (value: string): WValue => ({ type: "Text", value });
const record = (fields: [string, WValue][]): WValue => ({ type: "Record", value: fields });

describe("who decides an approval", () => {
  it("decides by their name, the local admin by the name it runs as", () => {
    expect(deciderOf(ada)).toBe("ada");
    expect(deciderOf({ name: "local:ada", kind: "person", role: "admin" })).toBe("ada");
  });
  it("takes the steps the server says are theirs, and says the one they decided", () => {
    expect(yourSteps(alone(2, [], [0])).map((s) => s.slot)).toEqual([0]);
    expect(yourSteps(chain(["scout"], [], [1])).map((s) => s.role)).toEqual(["reviewer"]);
    expect(yourSteps(chain(["scout"], [], []))).toEqual([]);
    expect(yourSteps({ name: "paid", expires_at: 0 })).toEqual([]);
    expect(yourDecision(alone(2, ["ada"]), ada)).toBe("you approved");
    expect(yourDecision(chain(["scout"], ["ada"]), ada)).toBe("you approved as reviewer");
    expect(yourDecision(alone(2, [], [0]), { name: "local:ada", kind: "person", role: "admin" })).toBeNull();
    expect(yourDecision({ ...alone(2), decisions: [took("ada", 0, undefined, false)] }, ada)).toBe("you declined");
  });
  it("counts the approvals with a step still theirs", () => {
    const run = (...waiting: WaitingPoint[]): { slug: string; run: WaitingRun } => ({ slug: "payout", run: { run_id: "r", kind: "trigger", route: "pay", parked_at: 0, waiting, delivered: [] } });
    const runs = [run(alone(2, [], [0])), run(alone(2, ["ada"])), run({ name: "paid", expires_at: 0 }), run(chain(["scout"], [], [1]), alone(3, [], [0])), run(chain([], []))];
    expect(toDecide(runs)).toBe(3);
  });
});

describe("an approval, said", () => {
  it("says how many people, or each step as it stands", () => {
    expect(approvalSaid(alone(1))).toBe(`a person's approval "payout"`);
    expect(approvalSaid(alone(2))).toBe(`2 people's approval "payout"`);
    expect(approvalSaid(alone(3, ["ada", "bob"]))).toBe(`3 people's approval "payout" · 2 of 3: ada and bob approved`);
    expect(approvalSaid(chain([], []))).toBe(
      `approval "payout" (an agent holding classifier, then 2 people holding reviewer, then a person holding approver): classifier open; reviewer not open yet; approver not open yet`,
    );
    expect(approvalSaid(chain(["scout"], ["ada"]))).toBe(
      `approval "payout" (an agent holding classifier, then 2 people holding reviewer, then a person holding approver): classifier scout approved; reviewer 1 of 2: ada approved; approver not open yet`,
    );
  });
  it("says what it waits for once a step is approved", () => {
    const steps = (w: WaitingPoint) => w.slots ?? [];
    expect(waitsAfter(alone(2), steps(alone(2))[0])).toBe("1 more person after you");
    expect(waitsAfter(alone(3), steps(alone(3))[0])).toBe("2 more people after you");
    expect(waitsAfter(alone(2, ["bob"]), steps(alone(2, ["bob"]))[0])).toBeNull();
    const w = chain(["scout"], []);
    expect(waitsAfter(w, steps(w)[1])).toBe("1 more for reviewer and approver");
    const last = chain(["scout"], ["ada", "bob"]);
    expect(waitsAfter(last, steps(last)[2])).toBeNull();
  });
  it("says what a decision did", () => {
    const d = (outcome: Decided["outcome"], slot: number, w: WaitingPoint): Decided => ({ name: "payout", slot, slots: w.slots ?? [], outcome });
    expect(decidedSaid(d("waiting", 0, alone(2, ["dan"])))).toBe("Approved: 1 of 2. It waits for 1 more person.");
    expect(decidedSaid(d("approved", 0, alone(2, ["dan", "ada"])))).toBe("Approved by dan and ada: the run goes on.");
    expect(decidedSaid(d("declined", 0, alone(2)))).toBe("Declined: the run goes on as declined.");
    expect(decidedSaid(d("waiting", 0, chain(["scout"], [])))).toBe("Approved as classifier. It waits for reviewer (0 of 2) and approver.");
    expect(decidedSaid(d("waiting", 1, chain(["scout"], ["ada"])))).toBe("Approved as reviewer. It waits for reviewer (1 of 2) and approver.");
    expect(decidedSaid(d("declined", 1, chain(["scout"], [])))).toBe("Declined as reviewer: the run goes on as declined.");
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
