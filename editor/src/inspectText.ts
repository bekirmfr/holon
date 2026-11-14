// What the Inspector says of the place the text cursor is at, at the Statements level
// (web-console-L4.md §5.3): a name the flow imports (a database, a secret, a host…), a name the
// language knows (`post`, `route`, `approval`…), or the statement it is in. The cursor moves there
// as it is typed in, or as a row of the statement graph is picked.

import { lineAt } from "./canvasLevel";
import type { FlowGraph, GraphRow, Inspected } from "./holonText";
import { rowAtLine } from "./StatementGraph";

/** What a flow imports from outside itself (docs/LANGUAGE.md: "The outside world"). */
export type ImportKind = "secret" | "state" | "collection" | "folder" | "database" | "channel" | "mail" | "feed" | "queue" | "host" | "flow";

/** A name the text imports: what it is, its name on the host (`shop` of `database:shop`; for a
 *  flow's action, the flow), the line it is imported on, and whether it may go unset. */
export type Imported = { name: string; kind: ImportKind; target: string; line: number; optional: boolean };

const KINDS: ImportKind[] = ["secret", "collection", "folder", "database", "channel", "mail", "feed", "queue", "host", "flow"];

/** Each name the text imports from outside itself; the language's own (`"holon"`, `"holon:clock"`)
 *  are not. */
export function importsOf(text: string): Imported[] {
  const out: Imported[] = [];
  const re = /\bimport\s+(\{[^}]*\}|[A-Za-z_$][\w$]*)\s+from\s+["']([^"']*)["']([^\n]*)/g;
  for (const m of text.matchAll(re)) {
    const [, names, source, rest] = m;
    const line = lineAt(text, m.index ?? 0);
    const optional = /optional\s*:\s*["']?true/.test(rest);
    const colon = source.indexOf(":");
    const kind = (source === "state" ? "state" : colon > 0 ? source.slice(0, colon) : "") as ImportKind;
    if (kind !== "state" && !KINDS.includes(kind)) continue;
    const target = kind === "state" ? "state" : source.slice(colon + 1);
    const listed = names.startsWith("{")
      ? names
          .slice(1, -1)
          .split(",")
          .map((n) => n.trim().split(/\s+as\s+/).pop()!.trim())
          .filter(Boolean)
      : [names];
    for (const name of listed) out.push({ name, kind, target, line, optional });
  }
  return out;
}

/** The name the cursor is on, or just after: its word and where it starts, and whether a `.`
 *  comes before it (a member: `shop.one`'s `one`). */
export function wordAt(text: string, offset: number): { word: string; from: number; member: boolean } | null {
  const ident = /[\w$]/;
  let from = Math.min(offset, text.length);
  while (from > 0 && ident.test(text[from - 1])) from--;
  let to = Math.min(offset, text.length);
  while (to < text.length && ident.test(text[to])) to++;
  if (to === from || /\d/.test(text[from])) return null;
  return { word: text.slice(from, to), from, member: text[from - 1] === "." };
}

/** What the Inspector shows for the cursor: an imported name, a name the language knows, or the
 *  statement (or trigger) it is in. */
export type TextCase =
  | { case: "import"; imported: Imported }
  | { case: "word"; word: Inspected }
  | { case: "statement"; band: number; index: number; row: GraphRow };

/** A row of the statement graph picked, and where it put the cursor (the end of its mark). */
export type PickedRow = { band: number; index: number; to: number };

/** The case for the cursor at `offset` in `text`. `graph`: the text's statement graph (the last
 *  one drawn); `said`: the language's answer for the name there (holonText.ts's inspectAt);
 *  `picked`: the row picked last, which is the case while the cursor is where picking it put it.
 *  On a name the text binds itself, it is the statement the cursor is in. */
export function textCase(text: string, offset: number, graph: FlowGraph | null, said: Inspected | null, picked: PickedRow | null = null): TextCase | null {
  const w = wordAt(text, offset);
  if (w && !w.member) {
    const imported = importsOf(text).find((i) => i.name === w.word);
    if (imported) return { case: "import", imported };
  }
  const row = picked && offset === picked.to ? graph?.bands[picked.band]?.rows[picked.index] : undefined;
  if (row) return { case: "statement", band: picked!.band, index: picked!.index, row };
  const at = graph ? rowAtLine(graph, lineAt(text, offset)) : null;
  const statement = at ? ({ case: "statement", band: at.band, index: at.index, row: graph!.bands[at.band].rows[at.index] } as const) : null;
  if (said) {
    const bound = new Set((graph?.bands ?? []).flatMap((b) => b.rows.filter((r) => !r.head).flatMap((r) => [r.name, ...r.parts.map((p) => p.name)])));
    if (!bound.has(said.word)) return { case: "word", word: said };
  }
  return statement;
}

/** What an imported name is, in words. */
export const IMPORT_WORD: Record<ImportKind, string> = {
  secret: "a secret",
  state: "this flow's state",
  collection: "a collection",
  folder: "a folder",
  database: "a database",
  channel: "a channel",
  mail: "a mail account",
  feed: "a feed",
  queue: "a queue",
  host: "a host it may call",
  flow: "another flow's action",
};

/** Whether a manifest item is done through `imported`: it names its target, as items do
 *  (`DbWrite("shop/orders")`, `HttpRequest("httpbin.org")`, `RunsAction("billing/charge")`), or,
 *  for the state, is a state item. */
export function through(item: string, imported: Imported): boolean {
  if (imported.kind === "state") return item.startsWith("State");
  const named = /^\w+\("(.*)"\)$/.exec(item)?.[1];
  const t = imported.target;
  return named !== undefined && (named === t || named.startsWith(`${t}/`) || named.startsWith(`${t}:`));
}
