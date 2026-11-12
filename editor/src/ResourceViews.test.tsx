// Resources' and Policy's views (ResourceViews.tsx, PolicyViews.tsx; web-console-L4.md §9.1, §9.3,
// §13): who uses a resource, with what a waiting version would add in amber; a server secret as
// its mark, never its value, its buttons disabled with the refusal for whoever may not bind; the
// policy file with line numbers; and the check of waiting versions against the policy.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { initSync } from "../wasm/holon_wasm.js";
import { PolicyCheck, PolicyText } from "./PolicyViews";
import { CollectionsTab, SecretsTable, UsedBy } from "./ResourceViews";
import type { DeploymentEntry, HostPolicy, Proposal, SecretInfo } from "./server";

vi.mock("./server", async (original) => ({
  ...(await original<typeof import("./server")>()),
  getSecretHistory: vi.fn(async () => ({ versions: [], events: [{ at: 1, name: "stripe_key", action: "set", version: 2, by: "mira" }] })),
  channelCounts: vi.fn(async (slug: string) => {
    if (slug === "hidden") throw new Error("hidden may not do that");
    return [{ name: "updates", kept: 7, subscribers: 1 }];
  }),
}));

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
});

async function render(node: React.ReactNode): Promise<HTMLDivElement> {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => root!.render(node));
  return container;
}

const waiting = (over: Partial<Proposal>): Proposal => ({ id: 1, slug: "stock-sheet", unit: "u", why: "", agent: true, at_ms: 0, needs: [], state: "waiting", version: 2, ...over });

describe("Resources' views", () => {
  it("says who uses a resource and how, and in amber what a waiting version would add", async () => {
    const el = await render(
      <UsedBy
        holders={[{ slug: "orders-api", status: "running", attention: null, uses: [{ use: "read", table: "orders", revoked: false, item: 'DbRead("shop/orders")' }] }]}
        waiting={[{ slug: "stock-sheet", version: 2, uses: [{ use: "write", table: "stock", revoked: false, item: 'DbWrite("shop/stock")' }] }]}
      />,
    );
    expect(el.textContent).toContain("orders-api reads orders");
    const amber = el.querySelector(".hl-used-by__waiting")!;
    expect(amber.textContent).toBe("stock-sheet v2 would write stock");
    // Without the grants (the host's to view), it says who may see, not that nobody uses it.
    await act(async () => root!.render(<UsedBy holders={[]} waiting={[]} known={false} />));
    expect(el.textContent).toBe("who uses it is shown to those who may view the host");
  });

  it("counts each channel's kept packs, a dash where the caller may not view the flow's state", async () => {
    const deployment = (slug: string) => ({ slug, unit: "u", has_state: false }) as unknown as DeploymentEntry;
    const grant = (slug: string, items: string[]) => ({ slug, status: "running" as const, attention: null, items, revoked: [] });
    const el = await render(
      <CollectionsTab deployments={[deployment("board"), deployment("hidden")]} grants={[grant("board", ['ChannelSend("updates")', 'ChannelSend("quiet")']), grant("hidden", ['ChannelSend("feed")'])]} />,
    );
    await act(async () => {});
    const items = (label: string) => el.querySelector(`tr[aria-label="${label}"] .hl-table__num`)!;
    expect(items("channel updates of board").textContent).toBe("7");
    expect(items("channel updates of board").getAttribute("title")).toContain("1 subscriber now");
    expect(items("channel quiet of board").textContent).toBe("0");
    expect(items("channel feed of hidden").textContent).toBe("—");
  });

  it("shows a server secret's mark, never a value, and its buttons follow can", async () => {
    const secrets: SecretInfo[] = [{ name: "stripe_key", version: 2, source: "local", set_at: 1_700_000_000, to: ["api.stripe.com"] }];
    const refused = "a viewer only reads";
    const el = await render(
      <SecretsTable
        secrets={secrets}
        grants={[{ slug: "refunds", status: "running", attention: null, items: ['SecretRead("stripe_key")'], revoked: [] }]}
        known
        proposals={[waiting({ slug: "payouts", version: 4, needs: ['SecretRead("stripe_key")'] })]}
        bind={refused}
        unbind={refused}
        onSetNew={() => undefined}
        onWhere={() => undefined}
        onRemove={() => undefined}
        onChanged={() => undefined}
      />,
    );
    const row = el.querySelector('[aria-label="server secret stripe_key"]')!;
    expect(row.querySelector(".hl-secret-mark")!.textContent).toBe("set · v2");
    expect(row.textContent).toContain("sealed here");
    expect(row.textContent).toContain("https://api.stripe.com");
    expect(row.textContent).toContain("refunds");
    expect(row.textContent).toContain("payouts v4 would read it");
    expect(row.textContent).toContain("mira");
    for (const name of ["Set a new value", "where…", "×"]) {
      const b = [...row.querySelectorAll("button")].find((x) => x.textContent === name)!;
      expect(b.disabled, name).toBe(true);
      expect(b.title, name).toBe(refused);
    }
    // Its history is anyone's to read who sees it.
    expect(row.querySelector<HTMLButtonElement>('[aria-label="versions of stripe_key"]')!.disabled).toBe(false);
  });
});

describe("Policy's views", () => {
  it("shows the policy file as read, a number beside each line; none without a file", async () => {
    const el = await render(<PolicyText text={"# the host\n[limits]\nmax_requests = 40\n"} />);
    const lines = [...el.querySelectorAll(".hl-policy-text__line")].map((l) => [l.querySelector(".hl-policy-text__n")!.textContent, l.lastElementChild!.textContent]);
    expect(lines).toEqual([
      ["1", "# the host"],
      ["2", "[limits]"],
      ["3", "max_requests = 40"],
    ]);
    await act(async () => root!.render(<PolicyText text={null} />));
    expect(el.textContent).toContain("no holon.toml");
  });

  it("checks the deployments and each waiting version against the policy", async () => {
    const policy: HostPolicy = { defaults: { max_requests: 50 }, ceilings: { max_requests: 500 }, auto: [], allow_plain_http: false, allow_private_addresses: false };
    const deployments = [{ slug: "refunds" }, { slug: "chat", attention: "it needs HttpRequest(\"api.anthropic.com\")" }] as DeploymentEntry[];
    const el = await render(
      <PolicyCheck policy={policy} deployments={deployments} proposals={[waiting({ slug: "refunds", version: 4, needs: ['HttpRequest("api.stripe.com")', "Limit(max_requests=900)"] })]} />,
    );
    expect(el.textContent).toContain("1 of 2 deployments run within it; chat can't run as granted");
    const items = [...el.querySelectorAll(".hl-policy-check__item")].map((i) => i.textContent);
    expect(items[0]).toMatch(/^\?refunds v4 would call https:\/\/api\.stripe\.com, which nothing in the policy refuses: a person approves it$/);
    expect(items[1]).toMatch(/^✕refunds v4 would .*max_requests.*, which the policy refuses: its ceiling for max_requests is 500$/);
  });
});
