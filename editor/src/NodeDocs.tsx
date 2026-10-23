// What a node does, for the person placing it: an ⓘ on each node's title, each palette row, and
// the component tab. A primitive's text comes from core (`describe`, through wasm), a template's
// from templates.ts, and a holon's (a placed composite) is said from what it is. The canvas reads
// it through a context, so no document ever stores it.

import { createContext, useContext, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { PrimitiveData } from "./PrimitiveNode";
import { TEMPLATE_DOCS, isTemplate } from "./templates";

/** A placed node's description, given what it is. */
export type DescribeNode = (data: Pick<PrimitiveData, "name" | "body" | "inputs" | "outputs">) => string | null;

/**
 * Describe nodes from core's per-primitive texts (`docs`), the template texts, and for a holon its
 * ports and (`innerCount`) how many nodes it holds.
 */
export function nodeDescriber(docs: Map<string, string>, innerCount: (hash: string) => number | null): DescribeNode {
  return (data) => {
    if (data.body.kind === "Primitive") return docs.get(data.name) ?? null;
    if (data.body.kind === "Template") return isTemplate(data.name) ? TEMPLATE_DOCS[data.name] : null;
    if (data.body.kind === "Composite") return holonText(data.inputs.map((p) => p.name), data.outputs.map((p) => p.name), innerCount(data.body.hash));
    return null;
  };
}

/** A holon, said from what it is: a flow grouped into one node, its size and its ports. */
export function holonText(inputs: string[], outputs: string[], inner: number | null): string {
  const list = (ports: string[]) => (ports.length === 0 ? "nothing" : ports.join(", "));
  const size = inner === null ? "" : ` of ${inner} node${inner === 1 ? "" : "s"}`;
  return `A holon: a whole flow${size} grouped into one node, the same unit wherever it's placed. Takes ${list(inputs)}; gives ${list(outputs)}. Whatever it reads or writes inside counts in this flow's permissions. Select it and expand (edit mode) to see inside.`;
}

export const NodeDocs = createContext<DescribeNode>(() => null);
export const useNodeDocs = () => useContext(NodeDocs);

/** What a node can ask the canvas to do for it: place a holon's newest version over it. */
export type NodeActionsValue = { updateHolon: (id: string) => void };
export const NodeActions = createContext<NodeActionsValue>({ updateHolon: () => {} });
export const useNodeActions = () => useContext(NodeActions);

/** An ⓘ that shows `text` while hovered: drawn on the page itself, so a node or a scrolling
 *  list never clips it. */
export function InfoIcon({ text }: { text: string | null }) {
  const at = useRef<HTMLSpanElement>(null);
  const [shown, setShown] = useState<{ x: number; y: number } | null>(null);
  if (!text) return null;
  const show = () => {
    const r = at.current?.getBoundingClientRect();
    if (!r) return;
    // Below the icon, kept inside the window.
    const width = 280;
    setShown({ x: Math.max(8, Math.min(r.right - width, window.innerWidth - width - 8)), y: r.bottom + 6 });
  };
  return (
    <span
      ref={at}
      className="hl-info"
      aria-label={text}
      onMouseEnter={show}
      onMouseLeave={() => setShown(null)}
      // A click on the icon never selects, drags or drops the node it sits on.
      onMouseDown={(e) => e.stopPropagation()}
    >
      ⓘ
      {shown &&
        createPortal(
          <div className="hl-info__tip" style={{ left: shown.x, top: shown.y }} role="tooltip">
            {text}
          </div>,
          document.body,
        )}
    </span>
  );
}
