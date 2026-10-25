// A flow as text (`.holon`, docs/LANGUAGE.md): written and read in any editor, diffed, reviewed,
// kept in git, and compiled through the same builder as the samples, against the live catalog, so
// a flow written here and the same flow drawn on the canvas are the same unit. The language
// (reading, lowering, printing) is holon-author's (crates/holon-author/src/lang/), through the
// wasm bridge: the same code the command line runs. Both of its forms are read; the expression
// form is written.

import { compile_holon, holon_complete, holon_format, holon_highlight, holon_hover, holon_mistakes } from "../wasm/holon_wasm.js";
import { fromHex } from "./flowDoc";
import { knownNames, restoreDocument } from "./docUnits";
import type { Evaluator } from "./holon";
import type { FlowDocument } from "./server";

/** A text that can't be read or built, and where (1-based line and column) it says why. */
export class HolonTextError extends Error {
  constructor(
    public line: number,
    public col: number,
    message: string,
  ) {
    super(`line ${line}, column ${col}: ${message}`);
  }
}

/** Where a node is written in a text: UTF-16 offsets, as CodeMirror counts them. */
export type TextRange = { from: number; to: number };

/** `.holon` text as a flow document (and its name); its holons' units are put into `ev`. Errors
 *  say the line and column. `names`: each top-level node's name in the text, by its id in the
 *  document (a binding's, or a `node` statement's); `lines`: the line (1-based) each is written
 *  on; `spans`: the range each is written over (docs/L4s/text-graph-link-L4.md). */
export function compileHolon(
  text: string,
  ev: Evaluator,
): { name: string | null; doc: FlowDocument; names: Map<string, string>; lines: Map<string, number>; spans: Map<string, TextRange> } {
  let compiled: { name: string | null; doc: FlowDocument; names: Record<string, string>; lines?: Record<string, number>; spans?: Record<string, [number, number]> };
  try {
    compiled = compile_holon(text);
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    const m = /^(\d+):(\d+): ([\s\S]*)$/.exec(message);
    throw m ? new HolonTextError(Number(m[1]), Number(m[2]), m[3]) : e;
  }
  for (const hex of compiled.doc.units ?? []) ev.insert(fromHex(hex));
  return {
    name: compiled.name,
    doc: compiled.doc,
    names: new Map(Object.entries(compiled.names)),
    lines: new Map(Object.entries(compiled.lines ?? {})),
    spans: new Map(Object.entries(compiled.spans ?? {}).map(([id, [from, to]]) => [id, { from, to }])),
  };
}

type Printable = Pick<FlowDocument, "nodes" | "edges"> & Partial<FlowDocument>;

/** A flow document as `.holon` text in the expression form: its settings, its holons (each with
 *  its boundary given, so it compiles back to the same unit), its routes or statements. Positions
 *  are not part of a program. `bindings` names top-level nodes (from `compileHolon`), so a text
 *  imported and exported keeps its names. */
export function printHolon(name: string | null, doc: Printable, ev: Evaluator, bindings?: Map<string, string>): string {
  restoreDocument(doc, ev);
  return ev.print_holon(name ?? undefined, doc, knownNames(), bindings ? Object.fromEntries(bindings) : undefined);
}

/** `printHolon`, and the line (1-based) each top-level node is written on, by id: an inline
 *  node's is its consumer's statement, a route's request node's is the route header. */
export function printHolonLines(name: string | null, doc: Printable, ev: Evaluator, bindings?: Map<string, string>): { text: string; lines: Map<string, number> } {
  restoreDocument(doc, ev);
  const printed: { text: string; lines: Record<string, number> } = ev.print_holon_lines(name ?? undefined, doc, knownNames(), bindings ? Object.fromEntries(bindings) : undefined);
  return { text: printed.text, lines: new Map(Object.entries(printed.lines)) };
}

/** The line of `nodeId` in the flow's text, and the text of that line, for the Inspector: null
 *  when the flow can't be printed (an unfinished canvas, say). */
export function sourceLineOf(nodeId: string, name: string | null, doc: Printable, ev: Evaluator): { line: number; text: string } | null {
  try {
    const { text, lines } = printHolonLines(name, doc, ev);
    const line = lines.get(nodeId);
    if (!line) return null;
    return { line, text: text.split("\n")[line - 1] ?? "" };
  } catch {
    return null;
  }
}

// ── What an editor asks of a text (the text pane; holon-author's editing.rs, as `holon lsp`
// answers it). Positions are UTF-16 offsets into the whole text, as the editor counts them; the
// bridge converts them to the language's lines and columns.

/** A stretch of the text: from an offset to an exclusive end. */
export type Stretch = { from: number; to: number };

/** The text's mistakes, as `holon check` says them. */
export const textMistakes = (text: string): (Stretch & { message: string })[] => holon_mistakes(text);

/** What the word at `offset` is (markdown), or null. */
export const textHover = (text: string, offset: number): (Stretch & { text: string }) | null => holon_hover(text, offset) ?? null;

/** What may be written at `offset`, starting with the word typed before it. */
export const textCompletions = (
  text: string,
  offset: number,
): { label: string; kind: "keyword" | "function" | "variable" | "module" | "member"; detail: string; doc: string | null }[] => holon_complete(text, offset);

/** The text as `holon fmt` writes it; throws why it can't be. */
export const textFormatted = (text: string): string => holon_format(text);

/** The text's colours, in order: each stretch with what it is (editing.rs's `Class`, lowercase). */
export const textColours = (text: string): (Stretch & { class: string })[] => holon_highlight(text);
