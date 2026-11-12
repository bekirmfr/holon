// A flow as text (`NAME.holon.ts`, docs/LANGUAGE.md): written and read in any editor, diffed,
// reviewed, kept in git, and compiled through the same builder as the samples, against the live
// catalog, so a flow written here and the same flow drawn on the canvas are the same unit. The
// language (reading, lowering, printing) is holon-author's (crates/holon-author/src/lang/),
// through the wasm bridge: the same code the command line runs.

import { compile_holon, document_text, holon_complete, holon_format, holon_graph, holon_highlight, holon_hover, holon_mistakes, holon_usage } from "../wasm/holon_wasm.js";
import { knownNames, restoreDocument } from "./docUnits";
import type { Evaluator } from "./holon";
import type { FlowDocument } from "./server";
import { restoreUnits } from "./docUnits";
import { sameGraph, type Graph } from "./textSync";

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

/** A flow's text as a flow document (and its name); its holons' units are put into `ev`. Errors
 *  say the line and column. `names`: each top-level node's name in the text, by its id in the
 *  document (a binding's, or a `node` statement's); `lines`: the line (1-based) each is written
 *  on; `spans`: the range each is written over (docs/L4s/text-graph-link-L4.md). */
/** The names a text binds, by the id of the canvas node each stands for: what a deploy keeps as
 *  the document's `bindings`, so a run explains as the text reads (`key = req.headers[…]`, not
 *  `lookup_2 = …`). Null when the text doesn't compile, or isn't this canvas: then nothing is named
 *  rather than something misnamed. */
export function bindingsOf(source: string, canvas: Graph, ev: Evaluator): Record<string, string> | null {
  let compiled: ReturnType<typeof compileHolon>;
  try {
    compiled = compileHolon(source, ev);
  } catch {
    return null;
  }
  const toCanvas = sameGraph({ nodes: compiled.doc.nodes, edges: compiled.doc.edges } as Graph, canvas);
  const bindings: Record<string, string> = {};
  for (const [id, name] of compiled.names) {
    const on = toCanvas.get(id);
    if (on !== undefined) bindings[on] = name;
  }
  return Object.keys(bindings).length > 0 ? bindings : null;
}

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
  restoreUnits(compiled.doc, ev);
  return {
    name: compiled.name,
    doc: compiled.doc,
    names: new Map(Object.entries(compiled.names)),
    lines: new Map(Object.entries(compiled.lines ?? {})),
    spans: new Map(Object.entries(compiled.spans ?? {}).map(([id, [from, to]]) => [id, { from, to }])),
  };
}

type Printable = Pick<FlowDocument, "nodes" | "edges"> & Partial<FlowDocument>;

/** A saved or proposed document as text, wherever one is shown (a review, a diff, a tab opened):
 *  the text it was saved with, comments and all, when that compiles to the unit its canvas deploys
 *  as; else its canvas printed. A document carries its code twice, a canvas that deploys and a
 *  text for people, and whoever saved it wrote both, so the text is shown only where it is the
 *  canvas's code (holon-author's `text_of`, through the bridge: the same rule the command line and
 *  the agent tools read a document by). Never `doc.source` as it comes. */
export function documentText(name: string | null, doc: Printable): string {
  return document_text(name ?? "", doc);
}

/** The text a document was saved with, when it is its canvas's code (`documentText`); null when
 *  it has none, or has one that says something else, or can't be read. */
export function sourceOf(name: string | null, doc: Printable): string | null {
  if (!doc.source) return null;
  try {
    return documentText(name, doc) === doc.source ? doc.source : null;
  } catch {
    return null;
  }
}

/** A flow document as its text: its settings, its holons (each with its boundary given, so it
 *  compiles back to the same unit), its handlers. Positions
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

/** What the Inspector says of a name beside its hover (holon-author's usage.rs): the forms it is
 *  written in, each with what the language reference says it means, and a sample's lines using it. */
export type Usage = { forms: [string, string][]; example: { sample: string; lines: string[] } | null };
export const textUsage = (text: string, offset: number): Usage | null => holon_usage(text, offset) ?? null;

/** The name at the cursor, or just before it (one being typed ends where the cursor is), for the
 *  Inspector: the word, its hover and its usage; null where there is no name. */
export type Inspected = { word: string; hover: string; usage: Usage | null };
export function inspectAt(text: string, offset: number): Inspected | null {
  for (const at of [offset, offset - 1]) {
    if (at < 0) continue;
    const h = textHover(text, at);
    if (h) return { word: text.slice(h.from, h.to), hover: h.text, usage: textUsage(text, at) };
  }
  return null;
}

/** What may be written at `offset`, starting with the word typed before it. */
export const textCompletions = (
  text: string,
  offset: number,
): { label: string; kind: "keyword" | "function" | "variable" | "module" | "member"; detail: string; doc: string | null }[] => holon_complete(text, offset);

/** The text as `holon fmt` writes it; throws why it can't be. */
export const textFormatted = (text: string): string => holon_format(text);

/** The text's colours, in order: each stretch with what it is (editing.rs's `Class`, lowercase). */
export const textColours = (text: string): (Stretch & { class: string })[] => holon_highlight(text);

// ── A flow by its statements (holon-author's graph.rs, as `holon editor` draws it; web-console-L4.md
// §4): one band per trigger, each a row per statement, with the wires between them. The terminal
// and the web read the same function, so they can't draw a flow differently.

/** What a statement does, as its colour says: computes, reads or writes the flow's data, calls
 *  outside, answers, or waits (for people to approve it, or for an event). */
export type RowKind = "compute" | "data" | "outside" | "answer" | "waits";

/** A placed node, as a statement's detail: its name, its leaf, its params as set, and what feeds
 *  each input (`account.out`). */
export type GraphPart = { name: string; leaf: string; params: [string, string][]; inputs: [string, string][] };

/** A place in the text: 1-based line and column (in characters). */
export type TextPos = { line: number; col: number };

/** A statement (or, with every node, a node), or its band's trigger. */
export type GraphRow = {
  /** Its first and last line, 1-based. */
  line: number;
  last: number;
  /** Its binding, `state.n`, `return`, what it calls. */
  name: string;
  /** What its outermost nodes are (`db_one`), or the trigger's. */
  does: string;
  /** Its first line, trimmed. */
  text: string;
  /** Where the text says it, to put the cursor on when the row is picked; `at` is the same as
   *  offsets. */
  mark: { start: TextPos; end: TextPos };
  at: Stretch;
  /** The band's trigger: it heads the band. */
  head: boolean;
  /** It acts on the world, not only computes. */
  effect: boolean;
  kind: RowKind;
  /** The status it answers when its value is absent, where the text says one (`?? status(404)`). */
  fallback: string | null;
  /** The rows of its band it takes from, by index (only earlier ones). */
  from: number[];
  /** What it takes from outside its band, by name. */
  outside: string[];
  parts: GraphPart[];
};

/** One trigger and the statements it runs. */
export type GraphBand = { title: string; rows: GraphRow[] };

/** A flow by its statements, and how many nodes it has in all. */
export type FlowGraph = { bands: GraphBand[]; nodes: number };

/** The text's statement graph (`every`: each node a row of its own); throws why it has none. */
export const holonGraph = (text: string, every = false): FlowGraph => holon_graph(text, every);
