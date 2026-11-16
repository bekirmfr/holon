// A switch between a flow's two modes that reads the server first shows what it read only if
// nothing moved meanwhile (useModeSwitch.ts). The e2e's slow runner: view mode read a run again
// while the person clicked edit; the read answered after, and put view mode back over their
// edit. Each case here: the switches asked one after another, their reads held, and every order
// the reads may answer in. Whatever the order, the last switch asked is the one shown; a tab
// picked meanwhile is kept.

import { act, useRef, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { blankMeta, blankTab, type FlowMeta, type TabState } from "./tabs";
import type { FlowDocument, LogEntry } from "./server";
import type { Evaluator } from "./holon";
import { newHistory, snapshotOf } from "./useCanvasHistory";
import { useModeSwitch } from "./useModeSwitch";

/** The server's reads of a deployed document, held until answered (a saved flow's answers at
 *  once: it is read only after the document is). */
const held = vi.hoisted(() => [] as { what: string; answer: () => void }[]);
vi.mock("./server", () => {
  const hold = <T,>(what: string, value: T) => new Promise<T>((resolve) => held.push({ what, answer: () => resolve(value) }));
  const doc = { nodes: [], edges: [], viewport: { x: 0, y: 0, zoom: 1 }, server: { port: 8156 } };
  return {
    getDeployedDocument: (slug: string) => hold(`document ${slug}`, doc),
    getFlow: (name: string) => Promise.resolve({ name, document: doc }),
    getLog: (_slug: string, after?: number) => Promise.resolve({ entries: [{ seq: (after ?? 0) + 1 }] }),
  };
});
vi.mock("./docUnits", () => ({ restoreDocument: () => undefined }));

/** What a switch asks: view mode, on a run or none; edit mode; another tab picked. */
type Ask = { view: number | null } | "edit" | "other tab";

const TAB = 1;
const OTHER = 2;
let root: Root | null = null;
let container: HTMLDivElement | null = null;
/** The hook and what it shows, as the last render gave them. */
let now: { meta: FlowMeta; active: number; onView: (slug: string, seq?: number) => Promise<void>; onEdit: () => void; pick: (id: number) => void } | null = null;

function Shows({ editCopy }: { editCopy: boolean }) {
  const [meta, setMeta] = useState<FlowMeta>(() => blankMeta("ticker"));
  const [active, setActive] = useState(TAB);
  const beforeView = useRef<TabState["beforeView"]>(null);
  const viewDoc = useRef<FlowDocument | null>(null);
  const history = useRef(newHistory(snapshotOf([], [])));
  const restoring = useRef(false);
  const stash = useRef(new Map<number, TabState>());
  const set = (patch: Partial<FlowMeta>) => setMeta((m) => ({ ...m, ...patch }));
  const { onView, onMode } = useModeSwitch({
    flow: meta,
    ev: {} as Evaluator,
    deployments: [],
    tabs: {
      activeId: active,
      stash,
      captureTab: () => ({ ...blankTab(meta.name), meta, beforeView: beforeView.current }),
      showTab: (_id, t) => {
        // A tab opened to view keeps no edit copy: edit mode then reads the flow again.
        beforeView.current = editCopy ? t.beforeView : null;
        setMeta(t.meta);
      },
      tabOf: (name) => (name === meta.name ? active : null),
    },
    tabFromDocument: (name, _doc, saved) => ({ ...blankTab(name), meta: { ...blankMeta(name), saved } }),
    canvas: { beforeView, viewDoc, history, restoring, setNodes: () => undefined, setEdges: () => undefined, setViewport: () => undefined, bump: () => undefined },
    setters: {
      setViewing: (viewing) => set({ viewing }),
      setViewTrace: (viewTrace) => set({ viewTrace }),
      setInspectNode: (inspectNode) => set({ inspectNode }),
      setSelectedRun: (selectedRun) => set({ selectedRun }),
      setStep: (step) => set({ step }),
    },
    setError: () => undefined,
  });
  now = { meta, active, onView, onEdit: () => onMode("edit"), pick: setActive };
  return null;
}

/** Let what was answered run on, and render. */
const settle = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

/** Every order of `n` reads answering. */
function orders(n: number): number[][] {
  if (n === 0) return [[]];
  return orders(n - 1).flatMap((o) => Array.from({ length: n }, (_, i) => [...o.slice(0, i), n - 1, ...o.slice(i)]));
}

/** Where the editor is: the mode, the run viewed, the tab. */
const where = () => ({ viewing: now!.meta.viewing, run: (now!.meta.selectedRun as LogEntry | null)?.seq ?? null, tab: now!.active });

/** In view mode on the flow's tab, its first read answered. */
async function viewing(editCopy: boolean) {
  act(() => root!.render(<Shows editCopy={editCopy} />));
  void now!.onView("ticker");
  held.splice(0).forEach((h) => h.answer());
  await settle();
  expect(where()).toEqual({ viewing: "ticker", run: null, tab: TAB });
}

/** Ask each of `asks` in turn, its reads held; then answer them in `order`. */
async function asked(asks: Ask[], order: number[]) {
  for (const a of asks) {
    if (a === "edit") act(() => now!.onEdit());
    else if (a === "other tab") act(() => now!.pick(OTHER));
    else void now!.onView("ticker", a.view ?? undefined);
    await settle();
  }
  const reads = held.splice(0);
  expect(reads.length, reads.map((r) => r.what).join(", ")).toBe(order.length);
  for (const i of order) {
    reads[i].answer();
    await settle();
  }
}

/** Where the last switch asked puts the editor; a tab picked keeps whatever its own showed. */
function last(asks: Ask[]) {
  const a = asks[asks.length - 1];
  if (a === "edit") return { viewing: null, run: null, tab: TAB };
  if (a === "other tab") return { viewing: "ticker", run: null, tab: OTHER };
  return { viewing: "ticker", run: a.view, tab: TAB };
}

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  held.length = 0;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  root = null;
  container = null;
  now = null;
});

// Each: whether the tab keeps its edit copy (edit mode then reads nothing), the switches asked,
// and how many reads they hold.
const CASES: { name: string; editCopy: boolean; asks: Ask[]; reads: number }[] = [
  { name: "a run read again, then edit asked", editCopy: true, asks: [{ view: 7 }, "edit"], reads: 1 },
  { name: "two runs asked, answered either way", editCopy: true, asks: [{ view: 7 }, { view: 8 }], reads: 2 },
  { name: "edit asked of a tab opened to view, then a run", editCopy: false, asks: ["edit", { view: 9 }], reads: 2 },
  { name: "a run asked, then edit of a tab opened to view", editCopy: false, asks: [{ view: 9 }, "edit"], reads: 2 },
  { name: "a run asked, then another tab picked", editCopy: true, asks: [{ view: 7 }, "other tab"], reads: 1 },
];

describe("the last switch asked is the one shown, whatever order the reads answer in", () => {
  for (const c of CASES) {
    for (const order of orders(c.reads)) {
      it(`${c.name}: answered ${order.join(", ")}`, async () => {
        await viewing(c.editCopy);
        await asked(c.asks, order);
        expect(where()).toEqual(last(c.asks));
      });
    }
  }
});
