// The canvas's two levels (web-console-L4.md §5.2, §13), wired as the editor wires them
// (useCanvasLevel.ts with StatementGraph and TextPane, as App.tsx passes them): a sample opens at
// Statements; a row's click puts the text's cursor on its mark, and the cursor's line selects the
// row; `l` switches the level; a row opened (Enter) goes to Nodes with the canvas's nodes it
// stands for. In Edit, a flow that is deployed has the rows new against the version deployed
// marked. And every row of every sample stands for nodes of its text.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { act, useCallback, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { EditorView } from "@codemirror/view";
import { Evaluator, initSync } from "../wasm/holon_wasm.js";
import { compileRows, DEFAULT_LEVEL, keepLevel, levelOf, rowNodes } from "./canvasLevel";
import { structuralKey } from "./flowDoc";
import { compileHolon, holonGraph } from "./holonText";
import { getDeployedDocument } from "./server";
import { buildSample, samples } from "./samples";
import { StatementGraph } from "./StatementGraph";
import { askText, TextPane, type TextAsk } from "./TextPane";
import { tabFromDocument, type FlowMeta, type TabState } from "./tabs";
import { useCanvasLevel } from "./useCanvasLevel";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

vi.mock("./server", async (original) => ({ ...(await original<typeof import("./server")>()), getDeployedDocument: vi.fn() }));

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  // CodeMirror measures text; jsdom lays nothing out.
  const none = () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] }) as unknown as DOMRectList;
  Range.prototype.getClientRects = none;
  Range.prototype.getBoundingClientRect = () => new DOMRect();
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
  localStorage.clear();
});

/** The editor's canvas and text for one tab, wired as App.tsx wires them. */
function Editor({ tab, ev, opened, deployed = null }: { tab: TabState; ev: Evaluator; opened: string[][]; deployed?: { slug: string; unit: string } | null }) {
  const [flow, setFlowState] = useState<FlowMeta>(tab.meta);
  const setFlow = useCallback((u: (m: FlowMeta) => FlowMeta) => setFlowState(u), []);
  const showText = useCallback((ask: TextAsk) => void askText(ask), []);
  const onOpened = useCallback((ids: string[]) => void opened.push(ids), [opened]);
  const level = useCanvasLevel({
    flow,
    setFlow,
    nodes: tab.nodes,
    edges: tab.edges,
    canvasKey: structuralKey(tab.nodes, tab.edges),
    ev,
    tab: 0,
    enabled: true,
    inspecting: true,
    showText,
    onOpened,
    deployed,
  });
  return (
    <div data-level={level.level}>
      {level.level === "statements" && level.graph && <StatementGraph pane graph={level.graph} against={level.against} cursor={level.cursorLine} onPick={level.onPick} onOpen={level.onOpen} />}
      <TextPane text={flow.text.value ?? ""} readOnly={false} onEdit={() => undefined} onCursor={() => undefined} onCaret={level.setCaret} status="" width={400} onWidth={() => undefined} onClose={() => undefined} />
    </div>
  );
}

function open(name: string, text?: string, deployed: { slug: string; unit: string } | null = null) {
  const ev = new Evaluator();
  // A text given is kept with its document, as a save keeps it: the tab shows it as written.
  const doc = text === undefined ? buildSample(samples().find((s) => s.name === name)!, ev) : { ...compileHolon(text, ev).doc, source: text };
  const tab = tabFromDocument(name, doc, false, ev);
  tab.meta.text = { ...tab.meta.text, open: true };
  const opened: string[][] = [];
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<Editor tab={tab} ev={ev} opened={opened} deployed={deployed} />));
  const c = container;
  const view = () => EditorView.findFromDOM(c.querySelector<HTMLElement>(".cm-editor")!)!;
  return { c, tab, opened, view, level: () => c.firstElementChild!.getAttribute("data-level") };
}

const rows = (c: HTMLElement) => [...c.querySelectorAll<HTMLButtonElement>(".hl-sg__row")];
const key = (k: string, at: EventTarget = document.body) => act(() => void at.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true })));

describe("the canvas's level", () => {
  it("opens at the default, Statements, and keeps a flow's in this browser by its name", () => {
    expect(DEFAULT_LEVEL).toBe("statements");
    expect(levelOf("hello-api")).toBe(DEFAULT_LEVEL);
    keepLevel("hello-api", "nodes");
    expect(levelOf("hello-api")).toBe("nodes");
    expect(levelOf(null)).toBe(DEFAULT_LEVEL);
  });

  it("a row's click puts the text's cursor on its mark, and the cursor's line selects its row", () => {
    const { c, view } = open("guarded-api");
    expect(rows(c).length).toBeGreaterThan(0);
    const graph = holonGraph(view().state.doc.toString());
    const i = graph.bands[0].rows.findIndex((r) => r.name === "receipt");
    const row = graph.bands[0].rows[i];
    act(() => rows(c)[i].click());
    const sel = view().state.selection.main;
    expect([sel.from, sel.to]).toEqual([row.at.from, row.at.to]);
    expect(view().state.doc.sliceString(sel.from, sel.to)).toBe("receipt");
    expect(rows(c)[i].getAttribute("aria-current")).toBe("true");
    expect(rows(c).filter((r) => r.getAttribute("aria-current") === "true")).toHaveLength(1);
  });

  it("switches with l, and a row opened (Enter) shows its nodes at the Nodes level", () => {
    const { c, opened, level, tab } = open("hello-api");
    expect(level()).toBe("statements");
    key("l");
    expect(level()).toBe("nodes");
    expect(c.querySelector(".hl-sg")).toBeNull();
    key("l");
    expect(level()).toBe("statements");
    // The answer's row: Enter on it opens it, the canvas's nodes it stands for framed.
    const answer = rows(c).find((r) => r.textContent?.includes("return"))!;
    key("Enter", answer);
    expect(level()).toBe("nodes");
    expect(opened).toHaveLength(1);
    const ids = new Set(tab.nodes.map((n) => n.id));
    expect(opened[0].length).toBeGreaterThan(0);
    expect(opened[0].every((id) => ids.has(id))).toBe(true);
    expect(opened[0].some((id) => tab.nodes.find((n) => n.id === id)!.data.name === "respond")).toBe(true);
  });

  it("in Edit, marks the rows new against the version deployed, read once for it", async () => {
    const sample = samples().find((s) => s.name === "hello-api")!;
    vi.mocked(getDeployedDocument).mockReset().mockResolvedValue(buildSample(sample, new Evaluator()));
    // The draft: a statement more than the version deployed, in a route of its own.
    const draft = `${sample.text}
route.get("/bye/:name", (req) => {
  const who = req.params.name
  return \`Bye, \${who}\`
})
`;
    const { c } = open("hello-api", draft, { slug: "hello-api", unit: "ab".repeat(32) });
    await act(async () => {});
    expect(getDeployedDocument).toHaveBeenCalledTimes(1);
    expect(getDeployedDocument).toHaveBeenCalledWith("hello-api", "ab".repeat(32));
    const fresh = rows(c).filter((r) => r.getAttribute("data-new") === "true");
    expect(fresh.map((r) => r.querySelector(".hl-sg__name")!.firstChild!.textContent)).toEqual(["GET /bye/:name", "who", "return"]);
    expect(rows(c).length).toBeGreaterThan(fresh.length);
  });

  it("is typed past: a key typed in the text is the text's", () => {
    const { c, level } = open("hello-api");
    key("l", c.querySelector(".cm-content")!);
    expect(level()).toBe("statements");
  });
});

describe("a row's nodes", () => {
  for (const s of samples()) {
    it(`${s.name}: every row stands for nodes of its text`, () => {
      const graph = holonGraph(s.text);
      const compiled = compileRows(s.text, new Evaluator());
      expect(compiled, s.name).not.toBeNull();
      for (const band of graph.bands)
        for (const row of band.rows) {
          const ids = rowNodes(row, compiled!);
          expect(ids.length, `${band.title}: ${row.text}`).toBe(row.parts.length);
        }
    });
  }
});
