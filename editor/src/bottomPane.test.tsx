// The editor's bottom pane (web-console-L4.md §5.4, §13): its tabs in each mode, Problems' row
// going to its line in the text, Replay the draft's three cases (a table, a call the deployed
// version never made, a flow never deployed), and closing a changed tab three ways.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { act, useRef, useState, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { EditorView } from "@codemirror/view";
import { initSync } from "../wasm/holon_wasm.js";
import { EDIT_TABS, EditBottomPane, outcomeSaid, ProblemsTab, problemsOf, ReplayDraft, type EditTab } from "./BottomPane";
import { BottomPane as RunPane } from "./ViewPanes";
import { KEYMAP } from "./keymap";
import { askText, TextPane } from "./TextPane";
import { DialogProvider, useDialog } from "./Dialog";
import { CLOSE_ANSWERS, useTabs } from "./useTabs";
import { blankTab, freshTab, type FlowMeta, type TabState } from "./tabs";
import type { LogEntry, Replayed } from "./server";

// The run pane reads the flow's log and history from the server: here, two runs and no history.
const RUNS: LogEntry[] = [1, 2].map((seq) => ({
  seq,
  session: 1,
  at_ms: 1_700_000_000_000 + seq,
  duration_ms: 1.5,
  kind: "request",
  route: "GET /hello/:name",
  method: "GET",
  url: `/hello/run${seq}`,
  status: 200,
  request: null,
  sourced: [],
  state_before: {},
  outputs: [],
  performed: [],
  response_body: "hi",
  error: null,
}));
vi.mock("./useFlowLog", () => ({ useFlowLog: () => ({ entries: RUNS, chain: { head: "", breaks: [] }, tallies: {}, error: "" }) }));
vi.mock("./HistoryPane", async (original) => ({
  ...(await original<typeof import("./HistoryPane")>()),
  useHistory: () => ({ events: [], versions: [], error: "", refresh: () => undefined }),
  useRestore: () => ({ restore: async () => undefined, busy: false, note: null, setNote: () => undefined, modal: null }),
}));

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

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
  root = null;
  vi.useRealTimers();
  localStorage.clear();
});

function mount(el: ReactNode): HTMLDivElement {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(el));
  return container;
}

const tabs = (c: HTMLElement) => [...c.querySelectorAll<HTMLButtonElement>("[role=tab]")];
const shownPanels = (c: HTMLElement) => [...c.querySelectorAll<HTMLElement>("[role=tabpanel]")].filter((p) => !p.hidden).map((p) => p.getAttribute("aria-label"));

describe("the bottom pane's tabs", () => {
  it("in Edit: Problems with its count, Try a request, Replay the draft and Settings, one shown", () => {
    const picked: EditTab[] = [];
    const panels = Object.fromEntries(EDIT_TABS.map(([t, label]) => [t, <p key={t}>{label} here</p>])) as Record<EditTab, ReactNode>;
    const c = mount(<EditBottomPane shown tab="try" onTab={(t) => picked.push(t)} problems={2} panels={panels} onClose={() => undefined} />);
    expect(tabs(c).map((t) => t.textContent)).toEqual(["Problems2", "Try a request", "Replay the draft", "Settings"]);
    expect(tabs(c).map((t) => t.getAttribute("aria-selected"))).toEqual(["false", "true", "false", "false"]);
    expect(shownPanels(c)).toEqual(["Try a request"]);
    act(() => tabs(c)[3].click());
    expect(picked).toEqual(["settings"]);
  });

  it("hidden (b), keeps its tabs and what is in them", () => {
    const panels = Object.fromEntries(EDIT_TABS.map(([t]) => [t, <input key={t} aria-label={t} />])) as Record<EditTab, ReactNode>;
    const c = mount(<EditBottomPane shown={false} tab="try" onTab={() => undefined} problems={0} panels={panels} onClose={() => undefined} />);
    expect(c.querySelector<HTMLElement>("[role=region]")!.hidden).toBe(true);
    expect(c.querySelectorAll("input")).toHaveLength(4);
  });

  it("in View runs: the runs, and the run picked with its Details and Timeline", () => {
    const c = mount(
      <RunPane
        slug="hello-api"
        selected={RUNS[1]}
        onSelect={() => undefined}
        step={null}
        onStep={() => undefined}
        order={[]}
        nodes={[]}
        timings={new Map()}
        exchanges={[]}
        verdict={true}
        onPickNode={() => undefined}
        diagnosis={null}
        onShowNode={() => undefined}
        onFitToNodes={() => undefined}
        keys={false}
      />,
    );
    const pane = c.querySelector("[role=region][aria-label=run]")!;
    expect(pane.textContent).toContain("#1");
    expect(pane.textContent).toContain("#2");
    const buttons = [...pane.querySelectorAll("button")].map((b) => b.textContent);
    expect(buttons).toEqual(expect.arrayContaining(["Details", "Timeline", "● live"]));
    expect(pane.textContent).toContain("/hello/run2");
  });

  it("in View runs: the runs and the run picked side by side; a click picks, a double click frames its nodes, the edge between them is dragged and kept", () => {
    let pickFromElsewhere: (e: LogEntry) => void = () => undefined;
    const framed: string[][] = [];
    function Pane() {
      const [selected, setSelected] = useState<LogEntry | null>(null);
      pickFromElsewhere = setSelected;
      return (
        <RunPane
          slug="hello-api"
          selected={selected}
          onSelect={setSelected}
          step={null}
          onStep={() => undefined}
          order={selected ? ["request-0", "respond-1"] : []}
          nodes={[]}
          timings={new Map()}
          exchanges={[]}
          verdict={null}
          onPickNode={() => undefined}
          diagnosis={null}
          onShowNode={() => undefined}
          onFitToNodes={(ids) => void framed.push(ids)}
          keys={false}
        />
      );
    }
    const c = mount(<Pane />);
    const runs = c.querySelector<HTMLElement>("[role=region][aria-label=Runs]")!;
    const run = c.querySelector<HTMLElement>("[role=region][aria-label=Run]")!;
    // Both in sight at once: no tabs to choose between.
    expect(runs).not.toBeNull();
    expect(run).not.toBeNull();
    expect(c.querySelector("[role=tablist][aria-label='bottom tabs']")).toBeNull();
    // Live picked the newest run: the list keeps its tools, the run its Details and Timeline.
    expect(c.textContent).toContain("#2 · GET /hello/:name");
    expect([...runs.querySelectorAll("button")].map((b) => b.textContent)).toContain("● live");
    expect([...run.querySelectorAll("button")].map((b) => b.textContent)).toEqual(expect.arrayContaining(["Details", "Timeline"]));
    expect(run.textContent).toContain("/hello/run2");
    // A run clicked in the list shows beside it, the list still there; double-clicked, its nodes
    // are framed on the canvas.
    const row = [...runs.querySelectorAll<HTMLElement>("div")].find((d) => d.firstElementChild?.nextElementSibling?.textContent === "#1")!;
    act(() => row.click());
    expect(run.textContent).toContain("/hello/run1");
    expect(runs.isConnected).toBe(true);
    act(() => void row.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
    expect(framed).toEqual([["request-0", "respond-1"]]);
    // A run picked elsewhere (its address, a cell's history) shows the same way.
    act(() => pickFromElsewhere(RUNS[1]));
    expect(run.textContent).toContain("/hello/run2");
    // The edge between them is dragged, and its width kept for next time.
    const edge = c.querySelector<HTMLElement>(".hl-view__divider")!;
    act(() => {
      edge.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientX: 400 }));
      window.dispatchEvent(new MouseEvent("pointermove", { clientX: 300 }));
      window.dispatchEvent(new MouseEvent("pointerup", { clientX: 300 }));
    });
    expect(Number(localStorage.getItem("holon.logRightWidth"))).toBeGreaterThan(0);
  });

  it("is b in both modes, beside c, x and i", () => {
    const editor = KEYMAP.filter((b) => b.scope === "editor").flatMap((b) => b.keys);
    const view = KEYMAP.filter((b) => b.scope === "view").flatMap((b) => b.keys);
    expect(editor).toEqual(expect.arrayContaining(["c", "x", "i", "b"]));
    expect(view).not.toContain("b");
  });
});

describe("Problems", () => {
  const TEXT = 'import { flow, route } from "holon"\n\nflow("hello-api", { port: 8101 })\n\nroute.get("/hello/:name", (req) => nope)\n';

  it("lists the text's mistakes as holon check finds them, each with its line", () => {
    const found = problemsOf(TEXT);
    expect(found).toHaveLength(1);
    expect(found[0].line).toBe(5);
    expect(found[0].message).toContain('unknown name "nope"');
    expect(problemsOf("")).toEqual([]);
    expect(problemsOf(null)).toEqual([]);
  });

  it("a row's click goes to its line in the text", () => {
    const go = (p: { line: number; from: number }) => void askText({ do: "line", line: p.line, at: p.from });
    const c = mount(
      <>
        <ProblemsTab problems={problemsOf(TEXT)} checked="" onGo={go} />
        <TextPane text={TEXT} readOnly={false} onEdit={() => undefined} onCursor={() => undefined} status="" width={400} onWidth={() => undefined} onClose={() => undefined} />
      </>,
    );
    const view = EditorView.findFromDOM(c.querySelector<HTMLElement>(".cm-editor")!)!;
    expect(view.state.doc.lineAt(view.state.selection.main.head).number).toBe(1);
    const row = c.querySelector<HTMLButtonElement>('[aria-label="problems"] button')!;
    expect(row.textContent).toContain("line 5");
    act(() => row.click());
    const head = view.state.selection.main.head;
    expect(view.state.doc.lineAt(head).number).toBe(5);
    expect(view.state.doc.sliceString(head, head + 4)).toBe("nope");
  });

  it("says there are none when there are none", () => {
    const c = mount(<ProblemsTab problems={[]} checked="Checked: no mistakes." onGo={() => undefined} />);
    expect(c.textContent).toContain("Checked: no mistakes.");
    expect(c.textContent).toContain("No problems");
  });
});

describe("Replay the draft", () => {
  const replayed: Replayed = {
    unit: "ab",
    replayed: 3,
    same: 1,
    differ: 1,
    cant_tell: 1,
    runs: [
      { seq: 7, at_ms: 0, kind: "request", route: "GET /hello/:name", outcome: "same", status: 200, now_status: 200 },
      { seq: 8, at_ms: 0, kind: "request", route: "POST /orders", outcome: "differs", why: ["it answers 500, not 201"], status: 201, now_status: 500 },
      {
        seq: 9,
        at_ms: 0,
        kind: "request",
        route: "GET /quote",
        outcome: "cant_tell",
        why: ["it sends GET https://api.example.com/rates, which the run didn't: what it would be answered can't be told offline"],
        status: 200,
      },
    ],
  };

  it("never deployed, says so, and replays nothing", () => {
    const replay = vi.fn(async () => replayed);
    const c = mount(<ReplayDraft slug="hello-api" deployed={false} unit="ab" shown replay={replay} />);
    expect(c.textContent).toBe("No runs to replay yet: deploy it first, or try a request.");
    expect(replay).not.toHaveBeenCalled();
  });

  it("deployed, runs the draft on the newest runs: a row each, was and now, same or differs", async () => {
    vi.useFakeTimers();
    const replay = vi.fn(async () => replayed);
    const c = mount(<ReplayDraft slug="hello-api" deployed unit="ab" shown replay={replay} />);
    expect(c.textContent).toContain("replaying…");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(500);
    });
    expect(replay).toHaveBeenCalledWith("hello-api", "ab");
    expect(c.querySelector('[aria-label="replay summary"]')!.textContent).toBe("3 replayed · 1 same · 1 differs · 1 can't tell");
    const rows = [...c.querySelectorAll("tbody tr")].map((r) => [...r.querySelectorAll("td")].map((td) => td.textContent));
    expect(rows[0]).toEqual(["#9", "GET /quote", "200", "?", expect.stringContaining("no recorded answer")]);
    expect(rows[1]).toEqual(["#8", "POST /orders", "201", "500", "differsit answers 500, not 201"]);
    expect(rows[2]).toEqual(["#7", "GET /hello/:name", "200", "200", "same"]);
  });

  it("a call the deployed version never made has no recorded answer, and the row says so", () => {
    expect(outcomeSaid(replayed.runs[2])).toBe("no recorded answer");
    expect(outcomeSaid({ ...replayed.runs[2], why: ["it computes with a secret, a token or a tainted value, which the log never holds"] })).toBe("can't tell");
    expect(outcomeSaid(replayed.runs[0])).toBe("same");
  });

  it("replays only while shown", async () => {
    vi.useFakeTimers();
    const replay = vi.fn(async () => replayed);
    mount(<ReplayDraft slug="hello-api" deployed unit="ab" shown={false} replay={replay} />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(replay).not.toHaveBeenCalled();
  });
});

describe("closing a changed tab", () => {
  type Handle = { tabs: ReturnType<typeof useTabs>; setDirty: (d: boolean) => void };
  const tabNamed = (name: string): TabState => ({ ...freshTab(name), meta: { ...freshTab(name).meta, saved: true } });

  function Harness({ handle, save }: { handle: { current: Handle | null }; save: () => Promise<{ name: string; key: string } | null> }) {
    const dialog = useDialog();
    const [flow, setFlow] = useState<FlowMeta>(blankTab(null).meta);
    const [dirty, setDirty] = useState(false);
    const live = useRef({ nodes: [], edges: [] });
    const history = useRef(blankTab(null).history);
    const restoring = useRef(false);
    const idRef = useRef(0);
    const beforeView = useRef<TabState["beforeView"]>(null);
    const viewDoc = useRef<TabState["viewDoc"]>(null);
    const t = useTabs({
      flow,
      setFlow,
      live,
      dirty,
      getViewport: () => ({ x: 0, y: 0, zoom: 1 }),
      setViewport: () => undefined,
      history,
      restoring,
      bump: () => undefined,
      idRef,
      beforeView,
      viewDoc,
      setNodes: () => undefined,
      setEdges: () => undefined,
      resetProjectionView: () => undefined,
      showCanvas: () => undefined,
      choose: dialog.choose,
      save,
    });
    handle.current = { tabs: t, setDirty };
    return null;
  }

  function open(save: () => Promise<{ name: string; key: string } | null> = vi.fn(async () => ({ name: "a", key: "saved-a" }))) {
    const handle: { current: Handle | null } = { current: null };
    mount(
      <DialogProvider>
        <Harness handle={handle} save={save} />
      </DialogProvider>,
    );
    const h = () => handle.current!;
    act(() => h().tabs.showTab(null, tabNamed("a")));
    act(() => h().setDirty(true));
    return { h, save };
  }

  const answers = () => [...document.querySelectorAll<HTMLButtonElement>("[role=dialog] button")];
  const answer = async (label: string) => {
    await act(async () => answers().find((b) => b.textContent?.endsWith(label))!.click());
  };
  const close = async (h: () => Handle, id = h().tabs.activeId) => {
    await act(async () => void h().tabs.closeTab(id));
  };

  it("asks three ways, each with its letter, Save and close highlighted", async () => {
    const { h } = open();
    await close(h);
    expect(document.querySelector("[role=dialog]")!.textContent).toContain('"a" has unsaved changes.');
    expect(answers().map((b) => b.textContent)).toEqual(["kKeep it open", "dDiscard", "sSave and close"]);
    expect(answers()[2].getAttribute("aria-keyshortcuts")).toBe("S Enter");
    expect(CLOSE_ANSWERS).toEqual(["Keep it open", "Discard", "Save and close"]);
    await answer("Keep it open");
  });

  it("Keep it open: the tab stays, with its changes", async () => {
    const { h, save } = open();
    await close(h);
    await answer("Keep it open");
    expect(h().tabs.tabIds).toHaveLength(1);
    expect(save).not.toHaveBeenCalled();
  });

  it("Discard (or d): the tab closes, kept among the recently closed with its changes", async () => {
    const { h, save } = open();
    await close(h);
    await act(async () => void document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "d", bubbles: true, cancelable: true })));
    expect(h().tabs.tabIds).toHaveLength(0);
    expect(h().tabs.closed[0].meta.name).toBe("a");
    expect(h().tabs.closed[0].dirty).toBe(true);
    expect(save).not.toHaveBeenCalled();
  });

  it("Save and close: saved, then closed, kept as saved", async () => {
    const { h, save } = open();
    await close(h);
    await answer("Save and close");
    expect(save).toHaveBeenCalledTimes(1);
    expect(h().tabs.tabIds).toHaveLength(0);
    expect(h().tabs.closed[0].dirty).toBe(false);
    expect(h().tabs.closed[0].meta.savedKey).toBe("saved-a");
  });

  it("Save and close on a tab not shown: shown first, then saved and closed", async () => {
    const { h, save } = open();
    const a = h().tabs.activeId;
    act(() => h().tabs.showTab(null, tabNamed("b")));
    act(() => h().setDirty(false));
    const b = h().tabs.activeId;
    expect(b).not.toBe(a);
    await close(h, a);
    await answer("Save and close");
    expect(save).toHaveBeenCalledTimes(1);
    expect(h().tabs.tabIds).toEqual([b]);
  });

  it("a save that doesn't happen leaves the tab open", async () => {
    const { h } = open(vi.fn(async () => null));
    await close(h);
    await answer("Save and close");
    expect(h().tabs.tabIds).toHaveLength(1);
  });

  it("an unchanged tab closes without asking", async () => {
    const { h } = open();
    act(() => h().setDirty(false));
    await close(h);
    expect(document.querySelector("[role=dialog]")).toBeNull();
    expect(h().tabs.tabIds).toHaveLength(0);
  });
});
