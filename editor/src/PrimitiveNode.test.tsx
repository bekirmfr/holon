// A node looks the same with or without a run's data: its box is the design. Rendered in a real
// React Flow, a node as designed (editable, no run) and the same node in view mode with long
// values, a failure and values switched on must lay out identically: everything a run adds is an
// overlay (`data-overlay`), outside the node's layout. jsdom does no layout, so "identically" is
// checked on what decides it: the element tree, each element's sizing styles, and its text.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { ReactFlow, ReactFlowProvider } from "@xyflow/react";
import { PrimitiveNode, type PrimitiveData, type PrimitiveNodeType } from "./PrimitiveNode";
import type { WValue } from "./holon";

beforeAll(() => {
  // What React Flow needs from a browser that jsdom doesn't have.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement | null = null;
afterEach(() => {
  container?.remove();
  container = null;
});

function render(data: PrimitiveData): HTMLElement {
  container = document.createElement("div");
  container.style.width = "800px";
  container.style.height = "600px";
  document.body.appendChild(container);
  const node: PrimitiveNodeType = { id: "n", type: "primitive", position: { x: 0, y: 0 }, data };
  act(() => {
    createRoot(container!).render(
      <ReactFlowProvider>
        <ReactFlow nodes={[node]} edges={[]} nodeTypes={{ primitive: PrimitiveNode }} />
      </ReactFlowProvider>,
    );
  });
  const box = container.querySelector<HTMLElement>(".hl-node");
  if (!box) throw new Error("the node didn't render");
  return box;
}

const SIZING = ["width", "height", "minWidth", "maxWidth", "padding", "margin", "display", "fontSize", "lineHeight", "gap", "flex"] as const;

/** What decides a node's layout: its tree without overlays, sizing styles and text. */
function layout(el: Element): unknown {
  return [...el.childNodes].flatMap((child): unknown[] => {
    if (child.nodeType === Node.TEXT_NODE) return child.textContent?.trim() ? [child.textContent] : [];
    if (!(child instanceof HTMLElement) || child.hasAttribute("data-overlay")) return [];
    const sizing = Object.fromEntries(SIZING.map((k) => [k, child.style[k]]).filter(([, v]) => v));
    return [{ tag: child.tagName, class: child.className, sizing, children: layout(child) }];
  });
}

const long: WValue = {
  type: "Record",
  value: [
    ["id", { type: "Text", value: "evt_1792683473691_a_long_identifier_that_would_widen_anything" }],
    ["data", { type: "List", value: Array.from({ length: 40 }, (_, i) => ({ type: "Number", value: i }) as WValue) }],
  ],
};

describe("a node in view mode", () => {
  const designed = (name: string, extra: Partial<PrimitiveData> = {}): PrimitiveData => ({
    name,
    body: { kind: "Primitive" },
    inputs: [
      { name: "a", ty: "Text" },
      { name: "b", ty: "Text" },
    ],
    outputs: [{ name: "out", ty: "Text" }],
    params: [],
    variadic: name === "concat",
    minInputs: 2,
    ...extra,
  });

  it("lays out exactly as designed, whatever the run put in it", () => {
    for (const name of ["concat", "field"]) {
      const asDesigned = layout(render(designed(name)));
      const inView = render(
        designed(name, {
          readOnly: true,
          trace: {
            outputs: [long],
            state: "done",
            showValues: true,
            issue: { role: "origin", reason: 'no field "kind" (it has data, id, object, type, and a great many more besides)' },
          },
        }),
      );
      expect(layout(inView), name).toEqual(asDesigned);
      // …while the run's data is there, as overlays.
      expect(inView.querySelector("[data-overlay]")?.textContent).toContain("no field");
      expect([...inView.querySelectorAll("[data-overlay]")].some((o) => o.textContent === "{2 fields}")).toBe(true);
    }
  });

  it("shows no values when they're switched off", () => {
    const off = render(designed("field", { readOnly: true, trace: { outputs: [long], state: "done", showValues: false } }));
    expect([...off.querySelectorAll("[data-overlay]")].some((o) => o.textContent === "{2 fields}")).toBe(false);
  });
});
