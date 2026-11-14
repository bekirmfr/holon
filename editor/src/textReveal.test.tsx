// The text and the canvas find each other, both ways, on every sample's draft: a node picked on
// the canvas brings its range into the text's view (without moving one already showing), and the
// cursor on a node's range picks that node and asks the canvas to bring it into view. A long text
// (trading-bot) is where a mark off-screen looked like no link at all.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { act, useCallback, useRef, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import type { Edge } from "@xyflow/react";
import { Evaluator, initSync } from "../wasm/holon_wasm.js";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { structuralKey } from "./flowDoc";
import { buildSample, samples } from "./samples";
import { tabFromDocument, type FlowMeta, type TabState } from "./tabs";
import { useTextPane } from "./useTextPane";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  root = null;
});

type Pane = ReturnType<typeof useTextPane>;

function Probe({ tab, ev, out, seen }: { tab: TabState; ev: Evaluator; out: { current: Pane | null }; seen: string[][] }) {
  const [flow, setFlowState] = useState<FlowMeta>(tab.meta);
  const setFlow = useCallback((u: (m: FlowMeta) => FlowMeta) => setFlowState(u), []);
  const [nodes, setNodesState] = useState<PrimitiveNodeType[]>(tab.nodes);
  const live = useRef<{ nodes: PrimitiveNodeType[]; edges: Edge[] }>({ nodes: tab.nodes, edges: tab.edges });
  live.current = { nodes, edges: tab.edges };
  const setNodes = useCallback((u: PrimitiveNodeType[] | ((ns: PrimitiveNodeType[]) => PrimitiveNodeType[])) => setNodesState((ns) => (typeof u === "function" ? u(ns) : u)), []);
  const idRef = useRef(tab.idCounter);
  out.current = useTextPane({
    flow,
    setFlow,
    nodes,
    edges: tab.edges,
    canvasKey: structuralKey(nodes, tab.edges),
    live,
    setNodes,
    setEdges: () => undefined,
    ev,
    idRef,
    viewing: false,
    run: null,
    onPick: () => undefined,
    onCursorNodes: (ids) => void seen.push(ids),
  });
  return null;
}

describe("the text and the canvas find each other", () => {
  it("on every sample's draft: each node's range revealed near, and the cursor there picks it and brings it into view", () => {
    for (const sample of samples()) {
      const ev = new Evaluator();
      const tab = tabFromDocument(sample.name, buildSample(sample, ev), false, ev);
      tab.meta.text = { ...tab.meta.text, open: true };
      const out: { current: Pane | null } = { current: null };
      const seen: string[][] = [];
      root = createRoot(document.createElement("div"));
      act(() => root!.render(<Probe tab={tab} ev={ev} out={out} seen={seen} />));
      const ids = new Set(tab.nodes.map((n) => n.id));
      let revealed = 0;
      // Up to forty nodes, spread over the flow: the first, the last, and between.
      const step = Math.max(1, Math.ceil(tab.nodes.length / 40));
      for (const n of tab.nodes.filter((_, i) => i % step === 0 || i === tab.nodes.length - 1)) {
        act(() => out.current!.revealNode(n.id, true));
        const linked = out.current!.pane.linked!;
        expect(linked, sample.name).toBeDefined();
        const r = linked.reveal;
        // A node inside a holon's body has no range of its own: it is linked through its call. One
        // whose range is empty or backwards (a guard the core lowers from an `if`) has nothing to
        // mark or pick either: the pane leaves it out.
        if (r === null || r.to <= r.from) continue;
        revealed++;
        expect(linked.revealNear, `${sample.name} ${n.id}`).toBe(true);
        seen.length = 0;
        act(() => out.current!.pane.onCursor(r.from));
        // What the cursor picked holds the cursor and is no larger than the node's range (the
        // node, one written inside it, or one ending where it starts: textLink.ts's pickAt); and
        // that is what the canvas is asked to bring into view.
        expect(seen, `${sample.name} ${n.id}`).toHaveLength(1);
        const picked = seen[0];
        expect(picked.length, `${sample.name} ${n.id}`).toBeGreaterThan(0);
        for (const id of picked) {
          expect(ids.has(id), `${sample.name} ${id}`).toBe(true);
          act(() => out.current!.revealNode(id, true));
          const inner = out.current!.pane.linked!.reveal!;
          expect(inner.from <= r.from && r.from <= inner.to && inner.to - inner.from <= r.to - r.from, `${sample.name}: ${id} at ${n.id}`).toBe(true);
        }
      }
      // A draft whose text stands for its canvas links its nodes: none revealed is the bug.
      expect(revealed, sample.name).toBeGreaterThan(0);
      act(() => root!.unmount());
      root = null;
    }
  }, 300_000);

  it("a reveal from elsewhere (show the node) centres, as before", () => {
    const sample = samples().find((s) => s.name === "trading-bot")!;
    const ev = new Evaluator();
    const tab = tabFromDocument(sample.name, buildSample(sample, ev), false, ev);
    tab.meta.text = { ...tab.meta.text, open: true };
    const out: { current: Pane | null } = { current: null };
    root = createRoot(document.createElement("div"));
    act(() => root!.render(<Probe tab={tab} ev={ev} out={out} seen={[]} />));
    const id = tab.nodes.find((n) => n.data.name === "json_parse")!.id;
    act(() => out.current!.revealNode(id));
    expect(out.current!.pane.linked!.reveal).not.toBeNull();
    expect(out.current!.pane.linked!.revealNear).toBe(false);
  });
});
