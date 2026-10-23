// First batch of frontend tests (previously: zero automated coverage at all — confirmed
// directly, not assumed, when the question came up). Tests diagnostics.ts, extracted out of
// App.tsx specifically so these four functions could be tested without pulling in App.tsx's own
// top-level value import of Evaluator from ./holon (which imports the actual wasm package).
// Chosen as the first batch's centerpiece because encapErrorNodeIndices is the exact function
// whose staleness (still referencing renamed/removed WEncapError variants) was caught by manual
// code review earlier this session — a test asserting it handles every CURRENT variant would
// have caught that immediately instead of leaving it to be found by chance.

import { describe, it, expect } from "vitest";
import {
  boundaryLabels,
  isWEncapError,
  validationErrorNodeIndices,
  encapErrorNodeIndices,
  formatValue,
  formatOutputs,
  summarizeState,
  valueChip,
} from "./diagnostics";

describe("summarizeState", () => {
  it("sums up lists and records by size and shows short scalars", () => {
    const n = (value: number): WValue => ({ type: "Number", value });
    expect(
      summarizeState({
        todos: { type: "List", value: [n(1), n(2)] },
        count: n(3),
        user: { type: "Record", value: [["id", n(1)]] },
        note: { type: "Text", value: "a very long note that keeps going and going" },
      }),
    ).toBe('count: 3, note: "a very long note that …, todos: 2 items, user: 1 field');
    expect(summarizeState({})).toBe("empty");
    expect(summarizeState(null)).toBe("no saved state");
  });
});
import type { WPort, WEdge, WEncapError, WValidationError, WValue } from "./holon";

// Inlined rather than imported: importing a real VALUE (not a type) from holon.ts would evaluate
// its whole module at runtime, including its own import of the actual wasm package
// ("../wasm/holon_wasm.js", built for --target web, browser-oriented) — exactly the risk
// extracting diagnostics.ts out of App.tsx was for. Every other import here is `type`-only,
// which TypeScript erases entirely, so those carry no such risk.
const BOUNDARY = 4294967295;

const port = (name: string): WPort => ({ name, ty: "Number" });
const edge = (from_child: number, from_port: number, to_child: number, to_port: number): WEdge => ({
  from_child,
  from_port,
  to_child,
  to_port,
});

describe("boundaryLabels", () => {
  it("uses the bare name when there's no provenance and no collision", () => {
    expect(boundaryLabels([port("a"), port("b")], [null, null])).toEqual(["a", "b"]);
  });

  it("prefixes with provenance when a source node is known", () => {
    expect(boundaryLabels([port("a"), port("b")], ["add", "multiply"])).toEqual([
      "add · a",
      "multiply · b",
    ]);
  });

  it("falls back to occurrence-counting only when provenance itself doesn't separate two ports", () => {
    // Two unconfigured instances of the SAME primitive, both named "b" — provenance alone
    // ("add · b" twice) doesn't disambiguate, so the (2) tiebreaker is the honest last resort.
    expect(boundaryLabels([port("b"), port("b")], ["add", "add"])).toEqual(["add · b", "add · b (2)"]);
  });

  it("never applies the occurrence-count tiebreaker when provenance already disambiguates", () => {
    // Same port name, but DIFFERENT source nodes — "add · b" and "multiply · b" are already
    // distinct strings, so neither should get a spurious "(2)" suffix.
    expect(boundaryLabels([port("b"), port("b")], ["add", "multiply"])).toEqual([
      "add · b",
      "multiply · b",
    ]);
  });

  it("disambiguates a bare-name collision (no provenance at all) the same way", () => {
    expect(boundaryLabels([port("b"), port("b")], [null, null])).toEqual(["b", "b (2)"]);
  });

  it("handles three-way collisions with increasing suffixes", () => {
    expect(boundaryLabels([port("b"), port("b"), port("b")], [null, null, null])).toEqual([
      "b",
      "b (2)",
      "b (3)",
    ]);
  });
});

describe("isWEncapError", () => {
  it("accepts a well-shaped structured error", () => {
    expect(isWEncapError({ kind: "UnresolvedChild", child: 0, message: "x" })).toBe(true);
  });

  it("rejects anything missing kind or message", () => {
    expect(isWEncapError({ child: 0, message: "x" })).toBe(false); // no kind
    expect(isWEncapError({ kind: "UnresolvedChild", child: 0 })).toBe(false); // no message
    expect(isWEncapError({})).toBe(false);
  });

  it("rejects non-objects and null without throwing", () => {
    expect(isWEncapError(null)).toBe(false);
    expect(isWEncapError(undefined)).toBe(false);
    expect(isWEncapError("a string")).toBe(false);
    expect(isWEncapError(42)).toBe(false);
  });
});

describe("validationErrorNodeIndices", () => {
  const wedges = [edge(0, 0, 1, 0), edge(BOUNDARY, 0, 2, 0)];

  it("resolves the four simple child-carrying variants to [child]", () => {
    const cases: WValidationError[] = [
      { kind: "ChildOutOfRange", edge: 0, child: 5, message: "x" },
      { kind: "UnresolvedChild", child: 5, message: "x" },
      { kind: "OutputPortOutOfRange", edge: 0, child: 5, port: 0, message: "x" },
      { kind: "InputPortOutOfRange", edge: 0, child: 5, port: 0, message: "x" },
    ];
    for (const err of cases) expect(validationErrorNodeIndices(err, wedges)).toEqual([5]);
  });

  it("resolves DuplicateInputEdge to [to_child]", () => {
    const err: WValidationError = { kind: "DuplicateInputEdge", to_child: 3, to_port: 1, message: "x" };
    expect(validationErrorNodeIndices(err, wedges)).toEqual([3]);
  });

  it("resolves TypeMismatch via the wedges array, both real endpoints", () => {
    const err: WValidationError = {
      kind: "TypeMismatch",
      edge: 0, // wedges[0] = (0,0) -> (1,0), neither endpoint is BOUNDARY
      src: "Number",
      dst: "Text",
      message: "x",
    };
    expect(validationErrorNodeIndices(err, wedges)).toEqual([0, 1]);
  });

  it("filters out BOUNDARY from TypeMismatch's endpoints, doesn't just pass it through", () => {
    const err: WValidationError = {
      kind: "TypeMismatch",
      edge: 1, // wedges[1] = BOUNDARY -> 2
      src: "Number",
      dst: "Text",
      message: "x",
    };
    expect(validationErrorNodeIndices(err, wedges)).toEqual([2]);
  });

  it("returns [] for TypeMismatch when the edge index is out of range, not a crash", () => {
    const err: WValidationError = {
      kind: "TypeMismatch",
      edge: 99,
      src: "Number",
      dst: "Text",
      message: "x",
    };
    expect(validationErrorNodeIndices(err, wedges)).toEqual([]);
  });

  it("returns [] for the three variants with no attributable node (named limit, not an oversight)", () => {
    const cases: WValidationError[] = [
      { kind: "BoundaryInputOutOfRange", edge: 0, port: 0, message: "x" },
      { kind: "BoundaryOutputOutOfRange", edge: 0, port: 0, message: "x" },
      { kind: "Cycle", message: "x" },
    ];
    for (const err of cases) expect(validationErrorNodeIndices(err, wedges)).toEqual([]);
  });
});

describe("encapErrorNodeIndices", () => {
  const wedges: WEdge[] = [];

  it("resolves every simple child-carrying variant to [child] — the current names, not stale ones", () => {
    // Deliberately exercises the CURRENT variant names (ExposeChildOutOfRange/ExposePortInvalid,
    // not the old OutputChildOutOfRange/OutputPortInvalid; no AmbiguousDefaultOutput at all) —
    // this is exactly the check that would have caught the stale-names bug immediately.
    //
    // Typed via Extract<WEncapError, { child: number }>, not the bare union: WEncapError also
    // includes `Invalid` (no `child` field), so `err.child` below wouldn't type-check against
    // the full union even though every case here genuinely has one. Extract narrows to exactly
    // the variants that structurally carry `child`, still checked against the real union — a
    // misspelled `kind` or a missing field still fails to compile, the actual point of typing
    // this array explicitly at all.
    const cases: Array<Extract<WEncapError, { child: number }>> = [
      { kind: "UnresolvedChild", child: 1, message: "x" },
      { kind: "PromoteChildOutOfRange", child: 2, message: "x" },
      { kind: "PromotePortOutOfRange", child: 3, port: 0, message: "x" },
      { kind: "PromoteAlreadyBound", child: 4, port: 0, message: "x" },
      { kind: "DuplicatePromotion", child: 5, port: 0, message: "x" },
      { kind: "ExposeChildOutOfRange", child: 6, message: "x" },
      { kind: "ExposePortInvalid", child: 7, port: 0, message: "x" },
    ];
    for (const err of cases) {
      expect(encapErrorNodeIndices(err, wedges)).toEqual([err.child]);
    }
  });

  it("flattens Invalid through validationErrorNodeIndices, across multiple wrapped errors", () => {
    const err: WEncapError = {
      kind: "Invalid",
      errors: [
        { kind: "UnresolvedChild", child: 1, message: "x" },
        { kind: "DuplicateInputEdge", to_child: 2, to_port: 0, message: "x" },
        { kind: "Cycle", message: "x" }, // contributes nothing
      ],
      message: "x",
    };
    expect(encapErrorNodeIndices(err, wedges)).toEqual([1, 2]);
  });
});

describe("valueChip", () => {
  it("summarizes any value in a few characters, cutting nothing mid-structure", () => {
    const txt = (value: string): WValue => ({ type: "Text", value });
    expect(valueChip(null)).toBe("—");
    expect(valueChip({ type: "Number", value: 200 })).toBe("200");
    expect(valueChip({ type: "Number", value: 1 / 3 })).toBe("0.333333");
    expect(valueChip({ type: "Boolean", value: false })).toBe("false");
    expect(valueChip(txt("Ada"))).toBe('"Ada"');
    expect(valueChip(txt("payment_intent.succeeded"))).toBe('"payment_intent.…"');
    expect(valueChip(txt("‹secret:stripe_webhook_secret›"))).toBe("‹secret›");
    expect(valueChip(txt("‹token:paid›"))).toBe("‹token›");
    const rec: WValue = { type: "Record", value: [["id", txt("evt_1")], ["data", { type: "List", value: [] }]] };
    expect(valueChip(rec)).toBe("{2 fields}");
    expect(valueChip({ type: "List", value: [txt("a")] })).toBe("[1 item]");
    expect(valueChip({ type: "Effect", value: { sink: "store_write:notes", payload: rec } })).toBe("effect store_write");
    expect(valueChip({ type: "Sum", value: { arm: 0, payload: txt("x") } })).toBe('some "x"');
    expect(valueChip({ type: "Sum", value: { arm: 1, payload: { type: "List", value: [] } } })).toBe("none");
  });
});

describe("formatValue", () => {
  it("formats a record the way core's to_text does, fields in key order", () => {
    const rec: WValue = {
      type: "Record",
      value: [
        ["id", { type: "Number", value: 7 }],
        ["title", { type: "Text", value: "buy milk" }],
      ],
    };
    expect(formatValue(rec)).toBe('{id: 7, title: "buy milk"}');
    expect(formatValue({ type: "Record", value: [] })).toBe("{}");
  });
  it("formats every scalar plainly", () => {
    expect(formatValue({ type: "Number", value: 3 })).toBe("3");
    expect(formatValue({ type: "Boolean", value: true })).toBe("true");
    expect(formatValue({ type: "Boolean", value: false })).toBe("false");
    expect(formatValue({ type: "Text", value: "hi" })).toBe('"hi"');
  });

  it("formats a list recursively, comma-separated", () => {
    const v: WValue = {
      type: "List",
      value: [{ type: "Number", value: 1 }, { type: "Number", value: 2 }],
    };
    expect(formatValue(v)).toBe("[1, 2]");
  });

  it("includes an Effect's payload, not just its sink name", () => {
    // The actual regression: this used to be `effect(store_write:count)` with no way to see
    // the value that was written — exactly the case a deployed state-write flow's whole point
    // is showing. Caught by building a real counter flow and looking at its result, not by
    // this test — which is the point of adding this test now.
    const v: WValue = {
      type: "Effect",
      value: { sink: "store_write:count", payload: { type: "Number", value: 3 } },
    };
    expect(formatValue(v)).toBe("effect(store_write:count, 3)");
  });

  it("formats a Sum by its arm index, never guessing a Some/None label", () => {
    const some5: WValue = { type: "Sum", value: { arm: 0, payload: { type: "Number", value: 5 } } };
    const none: WValue = { type: "Sum", value: { arm: 1, payload: { type: "List", value: [] } } };
    expect(formatValue(some5)).toBe("sum#0(5)");
    expect(formatValue(none)).toBe("sum#1([])");
  });
});

describe("formatOutputs", () => {
  it("keeps the exact old wording for the common single-null-slot case", () => {
    // The overwhelming majority of flows have exactly one output — this must read identically
    // to how it always did, even though the underlying type widened to an array under it.
    expect(formatOutputs([null])).toBe("(no output)");
  });

  it("shows a single real value bare, no separators, matching the old single-output shape", () => {
    const v: WValue = { type: "Text", value: "Welcome Bekir!" };
    expect(formatOutputs([v])).toBe('"Welcome Bekir!"');
  });

  it("reads all-null as no output regardless of how many slots there are", () => {
    expect(formatOutputs([null, null])).toBe("(no output)");
  });

  it("comma-joins a genuine multi-output settle — the actual point of this whole change", () => {
    const greeting: WValue = { type: "Text", value: "Welcome Bekir!" };
    const effect: WValue = {
      type: "Effect",
      value: { sink: "store_write:name", payload: { type: "Text", value: "Bekir" } },
    };
    expect(formatOutputs([greeting, effect])).toBe(
      '"Welcome Bekir!", effect(store_write:name, "Bekir")',
    );
  });

  it("shows (none) for an absent slot sitting alongside a real one", () => {
    const v: WValue = { type: "Number", value: 1 };
    expect(formatOutputs([v, null])).toBe("1, (none)");
  });
});

