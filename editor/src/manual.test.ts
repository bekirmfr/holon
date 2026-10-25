// The manual as the docs tab shows it: every page is in its contents, every link lands (a page
// and a heading on it, or the repository on GitHub), every flow in it opens through the real core
// (the wasm package, loaded from disk), and every page renders with no markdown left showing.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { marked, type Token, type Tokens } from "marked";
import { initSync, Evaluator } from "../wasm/holon_wasm.js";
import { compileHolon } from "./holonText";
import { HOME, PAGES, anchorsOf, contents, flowsOf, resolve } from "./manual";
import { ManualPage } from "./ManualPage";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

/** Every link on a page, wherever it is (a list, a table, a heading). */
function linksOf(page: string): string[] {
  const out: string[] = [];
  const walk = (ts: Token[] | undefined) => {
    for (const t of ts ?? []) {
      if (t.type === "link") out.push((t as Tokens.Link).href);
      if (t.type === "list") for (const item of (t as Tokens.List).items) walk(item.tokens);
      if (t.type === "table") {
        const tb = t as Tokens.Table;
        for (const c of [...tb.header, ...tb.rows.flat()]) walk(c.tokens);
      }
      walk((t as { tokens?: Token[] }).tokens);
    }
  };
  walk(marked.lexer(PAGES.get(page)!));
  return out;
}

describe("the manual in the editor", () => {
  it("is built in, index and all", () => {
    expect(PAGES.has(HOME)).toBe(true);
    expect(PAGES.has("language/reference.md")).toBe(true);
    expect(PAGES.size).toBeGreaterThan(10);
  });

  it("lists every page in its contents, once", () => {
    const listed = contents().flatMap((s) => s.pages.map((p) => p.path));
    expect(new Set(listed).size).toBe(listed.length);
    expect([...listed].sort()).toEqual([...PAGES.keys()].filter((p) => p !== HOME).sort());
    expect(contents().map((s) => s.title)).toEqual(["Start here", "The language", "Guides", "Reference"]);
  });

  it("resolves links within the manual and out of it", () => {
    expect(resolve("guides/files.md", "../concepts.md#the-run-record")).toEqual({ page: "concepts.md", anchor: "the-run-record" });
    expect(resolve("guides/files.md", "#sending-a-file")).toEqual({ page: "guides/files.md", anchor: "sending-a-file" });
    expect(resolve(HOME, "language/tour.md")).toEqual({ page: "language/tour.md", anchor: null });
    expect(resolve(HOME, "../README.md")).toEqual({ url: "https://github.com/bekirmfr/holon/blob/main/docs/README.md" });
    expect(resolve("language/reference.md", "../../LANGUAGE.md")).toEqual({ url: "https://github.com/bekirmfr/holon/blob/main/docs/LANGUAGE.md" });
    expect(resolve(HOME, "https://caddyserver.com")).toEqual({ url: "https://caddyserver.com" });
  });

  it("has no link that lands nowhere", () => {
    const wrong: string[] = [];
    for (const page of PAGES.keys()) {
      for (const href of linksOf(page)) {
        const to = resolve(page, href);
        if ("url" in to) continue;
        if (!PAGES.has(to.page)) wrong.push(`${page}: ${href}: no page ${to.page}`);
        else if (to.anchor && !anchorsOf(to.page).has(to.anchor)) wrong.push(`${page}: ${href}: no heading #${to.anchor}`);
      }
    }
    expect(wrong).toEqual([]);
  });

  it("opens every flow in it, as the docs tab's button does", () => {
    let seen = 0;
    const wrong: string[] = [];
    for (const page of PAGES.keys()) {
      for (const text of flowsOf(page)) {
        seen++;
        try {
          compileHolon(text, new Evaluator());
        } catch (e) {
          wrong.push(`${page}: ${e instanceof Error ? e.message : String(e)}\n${text}`);
        }
      }
    }
    expect(seen).toBeGreaterThan(20);
    expect(wrong).toEqual([]);
  });

  it("renders every page, with no markdown left showing", () => {
    for (const page of PAGES.keys()) {
      const html = renderToStaticMarkup(createElement(ManualPage, { at: { page, anchor: null }, onGo: () => {}, onTry: () => null }));
      // Outside code, where the manual writes markdown on purpose.
      const prose = html.replace(/<pre>[\s\S]*?<\/pre>|<code>[\s\S]*?<\/code>/g, "");
      expect(prose, page).not.toMatch(/\*\*|\]\(|^#+ /m);
      expect(html, page).not.toContain("&amp;lt;");
      const ids = [...html.matchAll(/ id="([^"]+)"/g)].map((m) => m[1]);
      expect(new Set(ids).size, page).toBe(ids.length);
      // The page's anchors are its headings' ids, so a link to one scrolls to it.
      expect(new Set(ids), page).toEqual(new Set([...anchorsOf(page)].map((a) => `manual-${a}`)));
    }
  });

  it("writes code as written, entities and all", () => {
    const html = renderToStaticMarkup(createElement(ManualPage, { at: { page: "language/reference.md", anchor: null }, onGo: () => {}, onTry: () => null }));
    // `a ?? b`, `<`, `&&` and quotes in the reference come out as the characters, escaped once.
    expect(html).toContain("??");
    expect(html).toMatch(/&lt;|&gt;/);
  });
});
