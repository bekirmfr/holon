// The Library's tutorials (tutorials.ts, LibraryPage.tsx; web-console-L4.md §10): the manual's
// pages that teach, each with its first paragraph and the samples it names, every one of which
// is a sample there is; a card's sample opens as an unsaved copy, through the same path the
// Samples tab takes.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { initSync } from "../wasm/holon_wasm.js";
import { LibraryPage } from "./LibraryPage";
import { PAGES, contents } from "./manual";
import { samples, type Sample } from "./samples";
import { firstParagraph, resolveSample, sampleMentions, tutorials } from "./tutorials";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = root = null;
});

describe("the tutorials", () => {
  const known = [
    { name: "ticker", title: "Ticker (a clock beside routes)" },
    { name: "price-rules", title: "Price rules (a new version replayed on recent runs)" },
    { name: "stripe-to-slack", title: "Stripe → Slack" },
    { name: "hello-api", title: "Hello API" },
  ];

  it("find the samples a page names, as it names them", () => {
    const text = [
      "The **ticker** sample (`holon new ticker.holon.ts --sample ticker`) has one.",
      "The **billing** and **shop** samples are this pair; the sample **rolling-log** keeps 5.",
      "As the approval-mail sample does. The `room` sample. Open the **Stripe → Slack** sample.",
      "A sample is a flow; **open** it. The follow up sample.",
    ].join("\n");
    expect(sampleMentions(text)).toEqual(["ticker", "ticker", "billing", "shop", "rolling-log", "approval-mail", "room", "Stripe → Slack"]);
  });

  it("know a sample by its name, its title, or its title before the parenthesis", () => {
    expect(resolveSample("ticker", known)).toBe("ticker");
    expect(resolveSample("Price rules", known)).toBe("price-rules");
    expect(resolveSample("Stripe → Slack", known)).toBe("stripe-to-slack");
    expect(resolveSample("Hello API", known)).toBe("hello-api");
    expect(resolveSample("Proposal", known)).toBeNull();
  });

  it("read a page's first paragraph, its marks without their syntax", () => {
    expect(firstParagraph("# Title\n\nA flow is **small**: see [the tour](tour.md) and `holon check`.\n\nMore.")).toBe("A flow is small: see the tour and holon check.");
  });

  it("are the manual's pages but its reference, each with its first paragraph", () => {
    const shown = tutorials(samples()).flatMap((s) => s.tutorials);
    const listed = contents().flatMap((s) => s.pages.map((p) => p.path));
    expect(shown.map((t) => t.path)).toEqual(listed.filter((p) => !/^reference\/|^language\/(reference|leaves)\.md$/.test(p)));
    expect(shown.map((t) => t.path)).toContain("getting-started.md");
    expect(shown.map((t) => t.path)).toContain("language/tour.md");
    for (const t of shown) expect(t.first, t.path).toMatch(/\w{3}/);
  });

  it("name only samples there are, and each page's samples resolve", () => {
    const names = new Set(samples().map((s) => s.name));
    const pages = tutorials(samples()).flatMap((s) => s.tutorials);
    for (const t of pages) {
      const unknown = sampleMentions(PAGES.get(t.path)!).filter((m) => resolveSample(m, samples()) === null);
      expect(unknown, t.path).toEqual([]);
      for (const n of t.samples) expect(names, t.path).toContain(n);
    }
    const by = new Map(pages.map((t) => [t.path, t.samples]));
    expect(by.get("getting-started.md")).toContain("hello-api");
    expect(by.get("guides/stripe-to-slack.md")).toEqual(["stripe-to-slack"]);
    expect(by.get("guides/actions.md")).toEqual(["ticker", "billing", "shop"]);
  });

  it("open a card's sample as the Samples tab does, and link its page in Docs", () => {
    const opened: Sample[] = [];
    const navigate = vi.fn();
    container = document.body.appendChild(document.createElement("div"));
    root = createRoot(container);
    act(() =>
      root!.render(
        <LibraryPage
          tab="tutorials"
          navigate={navigate}
          samples={samples()}
          itemsOf={() => []}
          onOpenSample={(s) => opened.push(s)}
          textOfSample={(s) => s.text}
          marketplace={{ url: "", onChange: () => {}, listings: [], adoption: () => 0, onYank: async () => "" }}
        />,
      ),
    );
    const card = container.querySelector<HTMLElement>('article[aria-label="Stripe to Slack"]')!;
    expect(card.textContent).toContain(firstParagraph(PAGES.get("guides/stripe-to-slack.md")!));
    act(() => card.querySelector<HTMLButtonElement>('button[aria-label="open Stripe → Slack"]')!.click());
    expect(opened.map((s) => s.name)).toEqual(["stripe-to-slack"]);
    act(() => card.querySelector<HTMLAnchorElement>("h3 a")!.click());
    expect(navigate).toHaveBeenCalledWith({ kind: "docs", page: "guides/stripe-to-slack.md", anchor: null });
  });
});
