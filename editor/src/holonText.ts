// A flow as text (`.holon`): written and read in any editor, diffed, reviewed, kept in git, and
// compiled through the same builder as the samples, against the live catalog, so a flow written
// here and the same flow drawn on the canvas are the same unit. The format (reading, compiling,
// printing) is holon-author's (crates/holon-author/src/text.rs), through the wasm bridge: the
// same code the command line runs.

import { compile_holon } from "../wasm/holon_wasm.js";
import { fromHex } from "./flowDoc";
import { knownNames, restoreDocument } from "./docUnits";
import type { Evaluator } from "./holon";
import type { FlowDocument } from "./server";

/** A text that can't be read or built, and the line (1-based) that says why. */
export class HolonTextError extends Error {
  constructor(
    public line: number,
    message: string,
  ) {
    super(`line ${line}: ${message}`);
  }
}

/** `.holon` text as a flow document (and its name); its holons' units are put into `ev`. Errors
 *  say the line. `names`: each top-level node's name in the text, by its id in the document. */
export function compileHolon(text: string, ev: Evaluator): { name: string | null; doc: FlowDocument; names: Map<string, string> } {
  let compiled: { name: string | null; doc: FlowDocument; names: Record<string, string> };
  try {
    compiled = compile_holon(text);
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    const m = /^line (\d+): ([\s\S]*)$/.exec(message);
    throw m ? new HolonTextError(Number(m[1]), m[2]) : e;
  }
  for (const hex of compiled.doc.units ?? []) ev.insert(fromHex(hex));
  return { name: compiled.name, doc: compiled.doc, names: new Map(Object.entries(compiled.names)) };
}

/** A flow document as `.holon` text: its settings, its holons (each with its boundary given, so
 *  it compiles back to the same unit), its nodes and wires. Positions are kept (`at`). */
export function printHolon(name: string | null, doc: Pick<FlowDocument, "nodes" | "edges"> & Partial<FlowDocument>, ev: Evaluator): string {
  restoreDocument(doc, ev);
  return ev.print_holon(name ?? undefined, doc, knownNames());
}
