// An Approvals card's rows (web-console-L4.md §7.2, §13): a proposal's card says its versions, its
// place in the queue, who proposed it and why, then the Review's rows that change, + new and − no
// longer, each with its line, and how many stay the same. They are read once the card comes into
// view, not before.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ProposalCard } from "./ApprovalsPage";
import { cardRows, mayGroups, textLines } from "./reviewMay";
import { getProposal, mayI, previewDeploy, setSession, type DeployPreview, type FlowDocument, type Proposal } from "./server";
import type { Touch } from "./touches";

vi.mock("./server", async (original) => ({ ...(await original<typeof import("./server")>()), getProposal: vi.fn(), previewDeploy: vi.fn(), mayI: vi.fn() }));

const TEXT = `import { flow, route } from "holon"
import api from "host:api.stripe.com"

flow("refunds", { port: 8301 })

route.post("/refund", async (req) => {
  return api.post("/v1/refunds", req.json)
})
`;

const touch = (kind: string, name: string, change: Touch["change"], approval: boolean, group = "outside", what = "call"): Touch => ({
  group,
  kind,
  name,
  change,
  uses: [{ item: `${kind}("${name}")`, what, words: "", change, approval, optional: false }],
  ready: null,
  has: null,
});
const TOUCHES = [
  touch("host", "api.stripe.com", "new", true),
  touch("host", "api.old.com", "gone", false),
  touch("requests", "", "same", false, "triggers", "receive"),
  touch("key", "count", "same", false, "state", "write"),
];

const proposal: Proposal = { id: 7, slug: "refunds", unit: "ab".repeat(32), why: "refund through Stripe", agent: true, by: "ci-bot", at_ms: 1_700_000_000_000, needs: ['HttpRequest("api.stripe.com")'], state: "waiting", version: 4 };

describe("an Approvals card's rows", () => {
  it("are the Review's: + new to approve, − no longer, each with its line, and the count of the same", () => {
    const rows = cardRows(mayGroups(TOUCHES, [], textLines(TEXT)));
    expect(rows.approve.map((r) => [r.line.text, r.anchor])).toEqual([["host api.stripe.com: call", 2]]);
    expect(rows.gone.map((r) => [r.line.text, r.anchor])).toEqual([["host api.old.com: call", null]]);
    expect(rows.same).toBe(2);
  });
});

describe("ProposalCard", () => {
  beforeAll(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  });
  let container: HTMLDivElement | null = null;
  let root: Root | null = null;
  // The cards that would come into view: the observer's callback, to say so.
  let shown: ((entries: { isIntersecting: boolean }[]) => void) | null = null;
  beforeEach(() => {
    setSession(null);
    vi.mocked(mayI).mockResolvedValue({ "flow.approve-deploy": true });
    vi.mocked(getProposal).mockResolvedValue({ ...proposal, document: { nodes: [], edges: [], viewport: { x: 0, y: 0, zoom: 1 }, source: TEXT } as FlowDocument });
    vi.mocked(previewDeploy).mockResolvedValue({ touches: TOUCHES, settings: [] } as unknown as DeployPreview);
    vi.stubGlobal(
      "IntersectionObserver",
      class {
        constructor(callback: (entries: { isIntersecting: boolean }[]) => void) {
          shown = callback;
        }
        observe() {}
        disconnect() {}
      },
    );
  });
  afterEach(() => {
    act(() => root?.unmount());
    container?.remove();
    container = null;
    vi.unstubAllGlobals();
    vi.mocked(getProposal).mockReset();
    vi.mocked(previewDeploy).mockReset();
  });

  it("says what it is, and reads its rows once it comes into view", async () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    const a = { kind: "update" as const, slug: "refunds", by: "ci-bot", at: 1_700_000_000, version: 4, proposal };
    const textOf = () => {
      throw new Error("no wasm here");
    };
    await act(async () => root!.render(<ProposalCard a={a} place={1} of={2} textOf={textOf} selected={false} navigate={() => undefined} onDecline={() => undefined} />));
    const card = container.querySelector("article")!;
    expect(card.textContent).toContain("v3 → v4");
    expect(card.textContent).toContain("1 of 2");
    expect(card.textContent).toContain("ci-bot");
    expect(card.textContent).toContain("refund through Stripe");
    expect(getProposal, "not read before it is in view").not.toHaveBeenCalled();

    await act(async () => shown!([{ isIntersecting: true }]));
    expect(getProposal).toHaveBeenCalledWith(7);
    expect(previewDeploy).toHaveBeenCalledWith("refunds", proposal.unit, undefined);
    const rows = [...card.querySelectorAll("li")].map((li) => li.textContent);
    // Its text as typed, when the document can't be printed here.
    expect(rows).toEqual(["+host api.stripe.com: callline 2", "−host api.old.com: call"]);
    expect(card.querySelector(".hl-approval-card__row--new")?.textContent).toContain("api.stripe.com");
    expect(card.textContent).toContain("2 the same");
    const buttons = [...card.querySelectorAll("button, a")].map((b) => b.textContent);
    expect(buttons).toEqual(["Decline with a reason…", "Review"]);
  });
});
