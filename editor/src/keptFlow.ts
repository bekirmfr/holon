// A flow as the server keeps it (docs/L4s/text-deploy-plan.md): its text, and where its canvas's
// nodes are when a person moved them (a layout). Every canvas the editor shows of a kept flow (a
// deployed version, a saved flow, a proposal, a run) is its text compiled here, through the same
// compiler the server and the command line compile it with (holon-author's `agent::canvas_of`,
// through the bridge), with its layout put back on it when it was drawn for that code. What is
// sent to be kept is the text (as typed, or the canvas printed) and the layout of the canvas.

import { kept_canvas, kept_layout_of } from "../wasm/holon_wasm.js";
import type { FlowDocument } from "./server";

/** Where a hand-drawn canvas's nodes are, handler by handler in its trace's order, and the code
 *  (unit, hex) it was drawn for (holon-author's `kept_layout::Layout`). */
export type Layout = { unit: string; at: Record<string, [number, number][]> };

/** What is sent to be kept: the text, and the layout when the canvas was moved by hand. */
export type KeptFlow = { text: string; layout: Layout | null };

/** A kept text's canvas, compiled here, with `layout` on it. Held to `unit` (the code it runs, when
 *  the server says): a text that compiles here to other code (another release's compiler) throws,
 *  since what is shown, and a run's steps laid on it, would be other code than runs. Throws the
 *  text's mistakes. */
export function keptDocument(text: string, layout?: Layout | null, unit?: string | null): FlowDocument {
  const kept = kept_canvas(text, layout ?? null) as { doc: FlowDocument; unit: string; placed: string | null };
  if (unit && kept.unit !== unit) {
    throw new Error(
      `its text compiles here to other code (${kept.unit.slice(0, 12)}) than it runs (${unit.slice(0, 12)}): this editor is another release than the one that compiled it; update whichever is older`,
    );
  }
  return kept.doc;
}

/** A saved flow's canvas: its text compiled ({@link keptDocument}); a draft whose text doesn't
 *  compile opens as its text alone, for its author to finish. */
export function savedDocument(text: string, layout?: Layout | null): FlowDocument {
  try {
    return keptDocument(text, layout);
  } catch {
    return { nodes: [], edges: [], source: text } as unknown as FlowDocument;
  }
}

/** What keeping `canvas` (a flow document with its `trace`) sends: `text` (as typed, or the canvas
 *  printed), and the canvas's layout. The text must be the canvas's code: when it compiles to
 *  other code than `unit` (the canvas's, when it has one), the canvas can't be kept as text, and
 *  this throws saying so: nothing is sent that would come back as another flow. */
export function flowToKeep(canvas: FlowDocument, text: string, unit: string | null): KeptFlow {
  const kept = kept_layout_of(canvas, text) as { unit: string; layout: Layout | null };
  if (unit && kept.unit !== unit) {
    throw new Error(
      `this canvas can't be written as text that runs as drawn: its text compiles to other code (${kept.unit.slice(0, 12)}) than the canvas (${unit.slice(0, 12)}). Open the text pane to see what it writes, and change the canvas or the text so they agree.`,
    );
  }
  return { text, layout: kept.layout };
}
