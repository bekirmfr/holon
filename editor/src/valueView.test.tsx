// A run's value shown in the editor runs nothing and loads nothing, under every renderer, for
// every value a drill-in reaches.
//
// Guards: `renderValueContent` (ViewPanes.tsx), what the value modal shows a run's value with,
// and the HTML renderer's frame (`inertHtml`, diagnostics.ts).
// Why: a run's values are anyone's: a request's body, a far side's answer, a mail, a file dropped
// in a folder. The modal picks a renderer from the value's own text (`detectRenderer`), so a
// value that starts `<!doctype` is shown as a page as soon as someone opens it.
// If it fails: a value someone sent runs script in the editor of whoever opens the run, or loads
// from a host of the sender's, telling them who looked and when.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { renderValueContent } from "./ViewPanes";
import { availableRenderers, detectRenderer, type RendererKind } from "./diagnostics";
import type { WValue } from "./holon";

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

// Every renderer, whether a value offers it or not: a kind added to RendererKind doesn't compile
// here until it has its row.
const RENDERERS: Record<RendererKind, true> = { raw: true, text: true, json: true, html: true, markdown: true, image: true, audio: true, table: true, record: true };

const t = (value: string): WValue => ({ type: "Text", value });
const SCRIPT = `<script>parent.postMessage("ran", "*")</script>`;
const LOADS = `<img src="https://x.test/i.png" onerror="alert(1)"><link rel="stylesheet" href="https://x.test/s.css"><style>@import url(https://x.test/i.css); body { background: url(https://x.test/b.png) }</style><iframe src="https://x.test/"></iframe><object data="https://x.test/o"></object><meta http-equiv="refresh" content="0;url=https://x.test/">`;
// Pages the renderer is offered for, each trying another way out.
const PAGES = [
  `<!doctype html><html><body onload="alert(1)">${SCRIPT}${LOADS}<p>shown</p></body></html>`,
  `  <!DOCTYPE html>\n<head><meta http-equiv="Content-Security-Policy" content="default-src *; script-src * 'unsafe-inline'"></head>${SCRIPT}<p>shown</p>`,
  `\uFEFF<!doctype html>${LOADS}<p>shown</p>`,
  `<html><base href="https://x.test/">${SCRIPT}<a href="javascript:alert(1)">shown</a><img src="relative.png"></html>`,
  `<!doctype html SYSTEM "about:legacy-compat">${"<b>".repeat(40)}${SCRIPT}shown`,
  `<!doctype html><svg onload="alert(1)"><script>alert(1)</script></svg><math><mtext><style><img src=https://x.test/m.png></style></mtext></math>shown`,
];
const OTHERS = [
  `data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"><script>alert(1)</script><image href="https://x.test/i.png"/></svg>`,
  "data:audio/wav;base64,UklGRiQAAABXQVZF",
  `# <script>alert(1)</script>\n**<img src=x onerror=alert(1)>** \`<iframe src=https://x.test/>\`\n\`\`\`html\n${SCRIPT}\n\`\`\``,
  JSON.stringify({ page: PAGES[0], link: "javascript:alert(1)" }),
  "javascript:alert(1)",
  SCRIPT,
];
const record = (fields: [string, WValue][]): WValue => ({ type: "Record", value: fields });
const texts = [...PAGES, ...OTHERS].map(t);
const POOL: WValue[] = [
  ...texts,
  record(texts.map((v, i) => [`f${i}`, v])),
  { type: "List", value: texts.map((v, i) => record([["n", { type: "Number", value: i }], ["body", v]])) },
  { type: "List", value: [record([["inner", record([["page", t(PAGES[1])], ["list", { type: "List", value: texts }]])]])] },
  { type: "Sum", value: { arm: 0, payload: t(PAGES[2]) } },
  { type: "Effect", value: { sink: "out", payload: record([["html", t(PAGES[3])]]) } },
];

/** A value and every value inside it: a drill-in reaches a record's fields, and from there on. */
function within(v: WValue): WValue[] {
  switch (v.type) {
    case "List": return [v, ...v.value.flatMap(within)];
    case "Record": return [v, ...v.value.flatMap(([, f]) => within(f))];
    case "Sum": return [v, ...within(v.value.payload)];
    case "Effect": return [v, ...within(v.value.payload)];
    default: return [v];
  }
}

/** A policy that lets a page load nothing but what it holds: nothing by default, and no source
 *  beyond `data:` (and inline styles) for any kind of load. */
function holdsOnlyItself(policy: string): boolean {
  const directives = new Map(policy.split(";").map((d) => d.trim().split(/\s+/)).filter((d) => d[0]).map(([name, ...sources]) => [name.toLowerCase(), sources]));
  if (directives.get("default-src")?.join(" ") !== "'none'") return false;
  return [...directives].every(([name, sources]) => sources.every((s) => s === "'none'" || s === "data:" || (name === "style-src" && s === "'unsafe-inline'")));
}

const URL_ATTRIBUTES = ["href", "src", "srcset", "action", "formaction", "data", "poster", "xlink:href"];

let root: Root | null = null;
let container: HTMLDivElement | null = null;
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  root = null;
  container = null;
});

function show(v: WValue, renderer: RendererKind): HTMLElement {
  act(() => root?.unmount());
  container?.remove();
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<>{renderValueContent(v, renderer, "value", () => {})}</>));
  return container;
}

describe("a run's value shown in the editor", () => {
  // Every renderer over every value, each rendered and parsed: a second alone, more beside a
  // whole run's other files, past vitest's five.
  it("runs no script and loads nothing, under every renderer, for every value a drill-in reaches", { timeout: 30_000 }, () => {
    const values = POOL.flatMap(within);
    const pages = new Set(PAGES);
    let frames = 0;
    for (const v of values) {
      // What the modal can show it with, and every other renderer besides.
      expect(availableRenderers(v)).toContain(detectRenderer(v));
      for (const renderer of Object.keys(RENDERERS) as RendererKind[]) {
        const shown = show(v, renderer);
        const at = `${renderer} of ${JSON.stringify(v).slice(0, 80)}`;
        expect(shown.querySelectorAll("script, object, embed, link, base, meta, frame").length, at).toBe(0);
        for (const el of shown.querySelectorAll("*")) {
          for (const a of el.attributes) {
            expect(a.name.toLowerCase().startsWith("on"), `${at}: ${el.tagName} ${a.name}`).toBe(false);
            if (URL_ATTRIBUTES.includes(a.name.toLowerCase())) expect(a.value.trimStart().toLowerCase().startsWith("data:"), `${at}: ${el.tagName} ${a.name}=${a.value}`).toBe(true);
          }
        }
        for (const frame of shown.querySelectorAll("iframe")) {
          frames += 1;
          // A frame with no flag runs no script, submits no form, opens nothing and navigates
          // nothing but itself, and refreshes nothing on its own.
          expect(frame.getAttribute("sandbox"), at).toBe("");
          expect(frame.hasAttribute("src") || frame.hasAttribute("allow"), at).toBe(false);
          // Its page: the policy before anything of the value's, so it holds for every load the
          // value's own markup asks for; and the value's text as it reads.
          const srcdoc = frame.getAttribute("srcdoc") ?? "";
          const doc = new DOMParser().parseFromString(srcdoc, "text/html");
          const first = doc.querySelector(":not(html):not(head)");
          expect(first?.tagName, at).toBe("META");
          expect(first?.getAttribute("http-equiv")?.toLowerCase(), at).toBe("content-security-policy");
          expect(holdsOnlyItself(first?.getAttribute("content") ?? ""), `${at}: ${first?.getAttribute("content")}`).toBe(true);
          const page = v.type === "Text" ? v.value : "";
          expect(doc.body.textContent, at).toBe(new DOMParser().parseFromString(page, "text/html").body.textContent);
        }
        if (renderer === "html" && v.type === "Text" && pages.has(v.value)) expect(shown.querySelectorAll("iframe").length, at).toBe(1);
      }
    }
    // The pages were shown as pages: the test held a frame for each, not nothing.
    expect(frames).toBeGreaterThanOrEqual(PAGES.length);
  });

  it("holds a policy to letting nothing load, as the test above reads one", () => {
    expect(holdsOnlyItself("default-src 'none'; style-src 'unsafe-inline'; img-src data:")).toBe(true);
    expect(holdsOnlyItself("default-src 'none'; img-src https:")).toBe(false);
    expect(holdsOnlyItself("default-src 'none'; script-src 'unsafe-inline'")).toBe(false);
    expect(holdsOnlyItself("img-src data:")).toBe(false);
  });
});
