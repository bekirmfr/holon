// An approval a run waits at, as the editor says it and which steps whoever is signed in takes
// (decisions.ts).
import { describe, expect, it } from "vitest";
import { approvalSaid, chainOf, chainSaid, decidedLine, decidedSaid, decisionSaid, decisionsOf, inSteps, shownSaid, toDecide, waitsAfter, yourDecision, yourSteps, type WaitingPoint, type WaitingStep } from "./decisions";
import type { WValue } from "./holon";
import type { Decided, WaitingDecision, WaitingRun } from "./server";

const took = (who: string, slot = 0, role?: string, approved = true) => ({ slot, who, kind: "person" as const, role, at: 0, approved });
const step = (slot: number, people: number, by: string[], open: boolean, role?: string, kind: "person" | "agent" = "person", starter = false): WaitingStep => ({
  slot,
  stage: slot,
  role,
  kind,
  people,
  ...(starter ? { starter } : {}),
  by,
  open,
});
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
/** An output review: an agent drafts it, then whoever started the run may approve it as editor. */
const review = (drafted: string[]): WaitingPoint => ({
  name: "shape",
  expires_at: 0,
  steps: "agent:drafter>person:editor^",
  said: "an agent holding drafter, then a person holding editor, who may be whoever started the run",
  slots: [step(0, 1, drafted, drafted.length === 0, "drafter", "agent"), step(1, 1, [], drafted.length > 0, "editor", "person", true)],
  decisions: drafted.map((b) => ({ ...took(b, 0, "drafter"), kind: "agent" as const })),
});
const ada = { name: "ada", kind: "person" as const, role: "editor" as const };
const t = (value: string): WValue => ({ type: "Text", value });
const record = (fields: [string, WValue][]): WValue => ({ type: "Record", value: fields });

describe("who decides an approval", () => {
  it("takes the steps the server says are theirs, and says the one they decided", () => {
    expect(yourSteps(alone(2, [], [0])).map((s) => s.slot)).toEqual([0]);
    expect(yourSteps(chain(["scout"], [], [1])).map((s) => s.role)).toEqual(["reviewer"]);
    expect(yourSteps(chain(["scout"], [], []))).toEqual([]);
    expect(yourSteps({ name: "paid", expires_at: 0 })).toEqual([]);
    expect(yourDecision(alone(2, ["ada"]), ada)).toBe("you approved");
    expect(yourDecision(chain(["scout"], ["ada"]), ada)).toBe("you approved as reviewer");
    expect(yourDecision(alone(2, ["ada"]), { name: "bob", kind: "person", role: "admin" })).toBeNull();
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

describe("an approval in steps, as its chain", () => {
  // Each row: the approval as it stands, and its chain in a line: a step taken ✓ with who took it
  // (and what their note says in a word or two), any other ◻, how far it is, or not open yet.
  const noted = (w: WaitingPoint, note: unknown): WaitingPoint => ({ ...w, decisions: (w.decisions ?? []).map((d) => (d.slot === 0 ? { ...d, note } : d)) });
  const rows: [string, WaitingPoint, string][] = [
    ["nothing taken: the first step open, the others not yet", chain([], []), "◻ classifier → ◻ reviewer (2 people, not open yet) → ◻ approver (not open yet)"],
    ["the agent's note, its label and confidence", noted(chain(["scout"], []), { label: "invoice", confidence: 0.93 }), "✓ classifier (scout, invoice, 0.93) → ◻ reviewer (2 people) → ◻ approver (not open yet)"],
    ["a note of anything else, not in the chain", noted(chain(["scout"], ["ada"]), { checked: ["po"] }), "✓ classifier (scout) → ◻ reviewer (1 of 2: ada) → ◻ approver (not open yet)"],
    ["a confidence alone", noted(chain(["scout"], ["ada", "bob"]), { confidence: 0.5 }), "✓ classifier (scout, 0.5) → ✓ reviewer (ada and bob) → ◻ approver"],
    ["a step whoever started the run may take, not open yet", review([]), "◻ drafter → ◻ editor (whoever started it may take it, not open yet)"],
    ["and open", review(["scout"]), "✓ drafter (scout) → ◻ editor (whoever started it may take it)"],
  ];
  it.each(rows)("%s", (_, w, said) => {
    expect(chainSaid(w)).toBe(said);
  });
  it("is drawn only for an approval in steps, each step with the decisions taken on it", () => {
    expect([inSteps(chain([], [])), inSteps(alone(2)), inSteps({ name: "paid", expires_at: 0 })]).toEqual([true, false, false]);
    // One step of people, which whoever started it may take, is a chain: the short form isn't.
    const theirs: WaitingPoint = { ...alone(1), steps: "person^", slots: [step(0, 1, [], true, undefined, "person", true)] };
    expect(inSteps(theirs)).toBe(true);
    expect(chainOf(chain(["scout"], ["ada"])).map((c) => [c.state, c.decisions.map((d) => d.who)])).toEqual([
      ["done", ["scout"]],
      ["open", ["ada"]],
      ["later", []],
    ]);
  });
});

describe("a decision, a line on the run's timeline", () => {
  const at = (s: number) => `t${s}`;
  // Each row: a decision as the server keeps it, and its line: when, who, what as which step, why,
  // what they noted, and the binding that let them with the access version to check it against.
  const rows: [string, WaitingDecision, string][] = [
    ["a person, the short form", { slot: 0, who: "ada", kind: "person", at: 5, approved: true }, "t5: ada approved"],
    [
      "an agent's step, with its note and binding",
      { slot: 0, who: "scout", kind: "agent", role: "classifier", at: 6, approved: true, note: { confidence: 0.93, label: "invoice" }, binding: 4, access: 7 },
      't6: scout (an agent) approved as classifier · noted {"confidence":0.93,"label":"invoice"} · through binding 4 (holon access show --version 7)',
    ],
    [
      "a decline with its reason",
      { slot: 1, who: "bob", kind: "person", role: "reviewer", at: 9, approved: false, reason: "not this vendor", binding: 5, access: 7 },
      "t9: bob declined as reviewer: not this vendor · through binding 5 (holon access show --version 7)",
    ],
    ["a step with no role, by its number", { slot: 2, who: "cy", kind: "person", at: 3, approved: true, reason: "checked" }, "t3: cy approved as slot 2: checked"],
  ];
  it.each(rows)("%s", (_, d, line) => {
    expect(decidedLine(d, at)).toBe(line);
  });
  it("reads each decision as the run's log holds it", () => {
    const num = (value: number): WValue => ({ type: "Number", value });
    const logged = (d: WaitingDecision): WValue => {
      const fields: [string, WValue | undefined][] = [
        ["slot", num(d.slot)],
        ["who", t(d.who)],
        ["kind", t(d.kind)],
        ["at", num(d.at)],
        ["approved", { type: "Boolean", value: d.approved }],
        ["role", d.role === undefined ? undefined : t(d.role)],
        ["reason", d.reason === undefined ? undefined : t(d.reason)],
        ["binding", d.binding === undefined ? undefined : num(d.binding)],
        ["access", d.access === undefined ? undefined : num(d.access)],
        ["note", d.note === undefined ? undefined : record([["confidence", num(0.93)], ["label", t("invoice")]])],
      ];
      return record(fields.filter((f): f is [string, WValue] => f[1] !== undefined).sort(([a], [b]) => (a < b ? -1 : 1)));
    };
    const decisions = rows.map(([, d]) => d);
    const given = record([
      ["approved", { type: "Boolean", value: false }],
      ["by", { type: "List", value: [] }],
      ["decisions", { type: "List", value: decisions.map(logged) }],
    ]);
    expect(decisionsOf(given)).toEqual(decisions);
    expect(decisionsOf(record([["method", t("GET")]]))).toEqual([]);
  });
});

describe("what an approval shows those who decide it", () => {
  // Each row: what the server lists (what the flow named, else the run's input), and what Runs
  // waiting says of it.
  const rows: [string, Partial<WaitingPoint>, { label: string; text: string } | null][] = [
    ["a record of plain values, a field each", { shows: { amount: 30, payee: "acme", urgent: true, memo: null } }, { label: "shows", text: "amount 30 · payee acme · urgent true · memo null" }],
    ["the run's input, when the flow names nothing", { input: { amount: 30 } }, { label: "started with", text: "amount 30" }],
    ["what the flow named, over the input", { shows: { amount: 30 }, input: { amount: 30, card: "x" } }, { label: "shows", text: "amount 30" }],
    ["anything deeper, as JSON", { shows: { lines: [1, 2], to: { iban: "DE00" } } }, { label: "shows", text: '{"lines":[1,2],"to":{"iban":"DE00"}}' }],
    ["a text, as it is: withheld, or too large", { shows: "‹withheld: computed from a secret›" }, { label: "shows", text: "‹withheld: computed from a secret›" }],
    ["a value named null, as null", { shows: null }, { label: "shows", text: "null" }],
    ["an empty record, as JSON", { input: {} }, { label: "started with", text: "{}" }],
    ["nothing, for a run a clock started", {}, null],
  ];
  it.each(rows)("%s", (_, listed, said) => {
    expect(shownSaid({ ...alone(1), ...listed })).toEqual(said);
  });
});
