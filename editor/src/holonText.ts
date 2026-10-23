// A flow as text (`.holon`): written and read in any editor, diffed, reviewed, kept in git, and
// compiled through the same builder as the samples (samples.ts), against the live catalog, so a
// flow written here and the same flow drawn on the canvas are the same unit. One statement a line:
//
//   flow hello-api                      the flow's name
//   port 8101                           deploy settings: port, interval, browsers, optional
//   node greet = concat {} at 40 120    a node: a catalog leaf, a template, or a holon defined
//                                       here; params in { }, an optional canvas position;
//                                       `concat[3]` for a variadic node with three inputs
//   hi.out -> greet.a                   a wire, from an output to an input
//   holon slugify(text) -> (slug) {     a holon: a flow of its own, placed as one node. Its
//     node lower = lowercase            boundary is proposed as "group" does, or given:
//     in.text -> lower.s                `in.<input> -> node.port`, `node.port -> out.<output>`;
//     ...                               a port's unit name, when it differs, follows in quotes:
//   }                                   `(text "lowercase · s")`
//   # a comment
//
// Names with spaces or dots are quoted ("lowercase · s"); values too, as JSON strings, when they
// aren't a plain word. Errors say the line.

import type { Edge } from "@xyflow/react";
import { buildSample, type Holon, type SampleBuilder } from "./samples";
import { innerCanvas, BOUNDARY_IN, BOUNDARY_OUT } from "./composites";
import { BOUNDARY } from "./holon";
import { isTemplate, templateDefaults, type TemplateName } from "./templates";
import { nameOf, restoreDocument } from "./docUnits";
import type { Evaluator, WUnit } from "./holon";
import type { FlowDocument } from "./server";
import type { PrimitiveNodeType } from "./PrimitiveNode";

// ── reading ──────────────────────────────────────────────────────────────────────────────────

type Port = { name: string; unit?: string };
type NodeStmt = { kind: "node"; line: number; name: string; of: string; arity?: number; params: Record<string, string>; at?: [number, number] };
type WireStmt = { kind: "wire"; line: number; from: [string, string]; to: [string, string] };
type HolonStmt = { kind: "holon"; line: number; name: string; inputs: Port[]; outputs: Port[]; body: Stmt[] };
type Stmt = NodeStmt | WireStmt | HolonStmt;

export type HolonFile = {
  name: string | null;
  settings: { port?: string; interval?: string; browsers?: string; optional: string[] };
  body: Stmt[];
};

export class HolonTextError extends Error {
  constructor(
    public line: number,
    message: string,
  ) {
    super(`line ${line}: ${message}`);
  }
}

/** One line's tokens: words, quoted strings (JSON), and punctuation. */
function tokens(text: string, line: number): string[] {
  const out: string[] = [];
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    if (c === "#") break;
    if (/\s/.test(c)) {
      i++;
    } else if (c === '"') {
      let j = i + 1;
      while (j < text.length && text[j] !== '"') j += text[j] === "\\" ? 2 : 1;
      if (j >= text.length) throw new HolonTextError(line, "a quote isn't closed");
      out.push(text.slice(i, j + 1));
      i = j + 1;
    } else if (text.startsWith("->", i)) {
      out.push("->");
      i += 2;
    } else if ("{}()[],:=.".includes(c)) {
      out.push(c);
      i++;
    } else {
      let j = i;
      while (j < text.length && !/[\s{}()[\],:=."#]/.test(text[j]) && !text.startsWith("->", j)) j++;
      out.push(text.slice(i, j));
      i = j;
    }
  }
  return out;
}

const unquote = (t: string, line: number): string => {
  if (!t.startsWith('"')) return t;
  try {
    return JSON.parse(t) as string;
  } catch {
    throw new HolonTextError(line, `${t} isn't a valid quoted string`);
  }
};

/** Parse `.holon` text: its statements, or the first line it can't read. */
export function parseHolon(text: string): HolonFile {
  const lines = text.split(/\r?\n/);
  const file: HolonFile = { name: null, settings: { optional: [] }, body: [] };
  const stack: Stmt[][] = [file.body];
  const holons: HolonStmt[] = [];
  lines.forEach((raw, index) => {
    const line = index + 1;
    const t = tokens(raw, line);
    if (t.length === 0) return;
    let at = 0;
    const peek = () => t[at];
    const next = (what: string) => {
      if (at >= t.length) throw new HolonTextError(line, `expected ${what}`);
      return t[at++];
    };
    const expect = (tok: string) => {
      const got = next(`"${tok}"`);
      if (got !== tok) throw new HolonTextError(line, `expected "${tok}", found "${got}"`);
    };
    const word = (what: string) => {
      const w = next(what);
      if ("{}()[],:=.".includes(w) || w === "->") throw new HolonTextError(line, `expected ${what}, found "${w}"`);
      return unquote(w, line);
    };
    const end = () => {
      if (at < t.length) throw new HolonTextError(line, `unexpected "${t[at]}"`);
    };
    const body = stack[stack.length - 1];
    const inHolon = stack.length > 1;
    const head = t[0];

    if (head === "}") {
      if (!inHolon) throw new HolonTextError(line, "a } without a holon to close");
      stack.pop();
      holons.pop();
      at = 1;
      return end();
    }
    if (!inHolon && ["flow", "port", "interval", "browsers", "optional"].includes(head)) {
      at = 1;
      if (head === "flow") file.name = word("the flow's name");
      else if (head === "optional") file.settings.optional.push(word("a permission, in quotes"));
      else if (head === "browsers") {
        const rest: string[] = [];
        while (at < t.length) {
          const w = next("an origin");
          if (w !== ",") rest.push(unquote(w, line));
        }
        file.settings.browsers = rest.join(" ");
      } else file.settings[head as "port" | "interval"] = word(`the ${head}`);
      return end();
    }
    if (head === "node") {
      at = 1;
      const name = word("the node's name");
      if (name === "in" || name === "out") throw new HolonTextError(line, `"${name}" names a holon's boundary: call the node something else`);
      expect("=");
      const of = word("what the node is (a leaf, a template or a holon)");
      let arity: number | undefined;
      if (peek() === "[") {
        at++;
        arity = Number(word("how many inputs"));
        if (!Number.isInteger(arity) || arity < 1) throw new HolonTextError(line, "inputs: a whole number");
        expect("]");
      }
      const params: Record<string, string> = {};
      if (peek() === "{") {
        at++;
        while (peek() !== "}") {
          const key = word("a param's name");
          expect(":");
          params[key] = word(`${key}'s value`);
          if (peek() === ",") at++;
          else if (peek() !== "}") throw new HolonTextError(line, `expected "," or "}" after ${key}'s value`);
        }
        at++;
      }
      let pos: [number, number] | undefined;
      if (peek() === "at") {
        at++;
        pos = [Number(word("x")), Number(word("y"))];
        if (pos.some((v) => !Number.isFinite(v))) throw new HolonTextError(line, "at: two numbers, x and y");
      }
      end();
      body.push({ kind: "node", line, name, of, arity, params, at: pos });
      return;
    }
    if (head === "holon") {
      if (inHolon) throw new HolonTextError(line, "a holon is defined at the top level (and placed anywhere)");
      at = 1;
      const name = word("the holon's name");
      const ports = (): Port[] => {
        expect("(");
        const list: Port[] = [];
        while (peek() !== ")") {
          const name = word("a port's name");
          const unit = peek()?.startsWith('"') ? unquote(next("its unit name"), line) : undefined;
          list.push(unit && unit !== name ? { name, unit } : { name });
          if (peek() === ",") at++;
          else if (peek() !== ")") throw new HolonTextError(line, `expected "," or ")" after ${name}`);
        }
        at++;
        return list;
      };
      const inputs = ports();
      expect("->");
      const outputs = ports();
      expect("{");
      end();
      const h: HolonStmt = { kind: "holon", line, name, inputs, outputs, body: [] };
      body.push(h);
      stack.push(h.body);
      holons.push(h);
      return;
    }
    // A wire: a.out -> b.in
    const end1 = (): [string, string] => {
      const node = word("a node");
      expect(".");
      return [node, word("a port")];
    };
    const from = end1();
    expect("->");
    const to = end1();
    end();
    body.push({ kind: "wire", line, from, to });
  });
  if (holons.length > 0) throw new HolonTextError(holons[holons.length - 1].line, `the holon "${holons[holons.length - 1].name}" isn't closed with }`);
  return file;
}

// ── compiling ────────────────────────────────────────────────────────────────────────────────

/** `.holon` text as a flow document (and its name), through the samples' builder; errors say
 *  the line. `names`: each top-level node's name in the text, by its id in the document. */
export function compileHolon(
  text: string,
  catalog: Map<string, WUnit>,
  variadic: Set<string>,
  ev: Evaluator,
): { name: string | null; doc: FlowDocument; names: Map<string, string> } {
  const file = parseHolon(text);
  const holons = new Map<string, Holon>();
  const names = new Map<string, string>();
  /** Run `f` for the statement on `line`: a builder's complaint says the line. */
  const at = <T>(line: number, f: () => T): T => {
    try {
      return f();
    } catch (e) {
      if (e instanceof HolonTextError) throw e;
      throw new HolonTextError(line, (e instanceof Error ? e.message : String(e)).replace(/^sample "[^"]*": /, ""));
    }
  };
  const replay = (b: SampleBuilder, stmts: Stmt[]) => {
    const ids = new Map<string, string>();
    for (const s of stmts) {
      if (s.kind === "node") {
        if (ids.has(s.name)) throw new HolonTextError(s.line, `there is a node "${s.name}" already`);
        const holon = holons.get(s.of);
        const id = at(s.line, () => {
          if (holon) {
            if (Object.keys(s.params).length > 0) throw new Error(`the holon "${s.of}" takes no params`);
            return b.place(holon, s.at);
          }
          const id = b.node(s.of, s.params, s.at);
          return id;
        });
        ids.set(s.name, id);
        // A variadic node's extra inputs, as many as it says.
        if (s.arity !== undefined) at(s.line, () => b.arity(id, s.arity!));
      } else if (s.kind === "wire") {
        const [fromNode, output] = s.from;
        const [toNode, input] = s.to;
        if (fromNode === "in" || toNode === "out") continue; // a holon's boundary: its block reads these
        const src = ids.get(fromNode) ?? (() => { throw new HolonTextError(s.line, `no node "${fromNode}" (nodes are declared before they're wired)`); })();
        const dst = ids.get(toNode) ?? (() => { throw new HolonTextError(s.line, `no node "${toNode}" (nodes are declared before they're wired)`); })();
        at(s.line, () => b.wire(src, output, dst, input));
      } else {
        throw new HolonTextError(s.line, "a holon is defined at the top level");
      }
    }
    return ids;
  };
  const doc = buildSample(
    {
      name: file.name ?? "untitled",
      title: file.name ?? "untitled",
      description: "",
      shows: [],
      tryIt: "",
      port: file.settings.port,
      interval: file.settings.interval,
      browsers: file.settings.browsers,
      optional: file.settings.optional.length ? file.settings.optional : undefined,
      build(b) {
        const top: Stmt[] = [];
        for (const s of file.body) {
          if (s.kind !== "holon") {
            top.push(s);
            continue;
          }
          if (holons.has(s.name)) throw new HolonTextError(s.line, `there is a holon "${s.name}" already`);
          const boundary = s.body.filter((x): x is WireStmt => x.kind === "wire" && (x.from[0] === "in" || x.to[0] === "out"));
          const holon = at(s.line, () => {
            if (boundary.length === 0) {
              return b.holon(s.name, (inner) => void replay(inner, s.body), { inputs: s.inputs.map((p) => p.name), outputs: s.outputs.map((p) => p.name) });
            }
            let ids = new Map<string, string>();
            const find = (w: WireStmt, node: string) => ids.get(node) ?? (() => { throw new HolonTextError(w.line, `no node "${node}" in the holon "${s.name}"`); })();
            return b.holonExact(
              s.name,
              (inner) => {
                ids = replay(inner, s.body);
              },
              { inputs: s.inputs, outputs: s.outputs },
              {
                get in() {
                  return boundary.filter((w) => w.from[0] === "in").map((w) => [w.from[1], find(w, w.to[0]), w.to[1]] as [string, string, string]);
                },
                get out() {
                  return boundary.filter((w) => w.to[0] === "out").map((w) => [find(w, w.from[0]), w.from[1], w.to[1]] as [string, string, string]);
                },
              },
            );
          });
          holons.set(s.name, holon);
        }
        for (const [name, id] of replay(b, top)) names.set(id, name);
      },
    },
    catalog,
    variadic,
    ev,
  );
  return { name: file.name, doc, names };
}

// ── writing ──────────────────────────────────────────────────────────────────────────────────

const PLAIN = /^[A-Za-z0-9_][A-Za-z0-9_\-/]*$/;
/** A name or value as text: bare when it is a plain word, else quoted. */
const q = (s: string) => (PLAIN.test(s) && !["node", "holon", "flow", "port", "interval", "browsers", "optional", "at", "in", "out"].includes(s) ? s : JSON.stringify(s));
/** A port's name as text: bare when a plain word (keywords are fine after a dot), else quoted. */
const qp = (s: string) => (PLAIN.test(s) ? s : JSON.stringify(s));
/** Names made unique, in order: a repeated one gets _2, _3… */
const unique = (names: string[]) => {
  const seen = new Set<string>();
  return names.map((n) => {
    let m = n;
    for (let k = 2; seen.has(m); k++) m = `${n}_${k}`;
    seen.add(m);
    return m;
  });
};
const ident = (s: string) => s.replace(/[^A-Za-z0-9_]/g, "_").replace(/^(\d)/, "_$1") || "node";

/** A flow document as `.holon` text: its settings, its holons (each with its boundary given, so
 *  it compiles back to the same unit), its nodes and wires. Positions are kept (`at`). */
export function printHolon(
  name: string | null,
  doc: Pick<FlowDocument, "nodes" | "edges"> & Partial<FlowDocument>,
  catalog: Map<string, WUnit>,
  variadic: Set<string>,
  ev: Evaluator,
): string {
  restoreDocument(doc, ev);
  const out: string[] = [];
  if (name) out.push(`flow ${q(name)}`);
  const s = doc.server;
  if (s?.port) out.push(`port ${q(s.port)}`);
  if (s?.interval) out.push(`interval ${q(s.interval)}`);
  if (s?.browsers) out.push(`browsers ${s.browsers.split(/[\s,]+/).filter(Boolean).map(q).join(" ")}`);
  for (const o of s?.requirements?.optional ?? []) out.push(`optional ${JSON.stringify(o)}`);

  // Holons, by hash: each written once, the ones inside before the ones that hold them.
  const holonNames = new Map<string, string>();
  const used = new Set<string>();
  const holonBlocks: string[] = [];
  const holonFor = (hash: string, display: { inputs: string[]; outputs: string[] }, hint: string): string => {
    const known = holonNames.get(hash);
    if (known) return known;
    let hname = ident(nameOf(hash) ?? hint);
    while (used.has(hname) || catalog.has(hname) || isTemplate(hname)) hname = `${hname}_`;
    used.add(hname);
    holonNames.set(hash, hname);
    const unit = ev.get_unit(hash) as WUnit;
    const unitIns = (unit.inputs ?? []).map((p) => p.name);
    const unitOuts = (unit.outputs ?? []).map((p) => p.name);
    // Open it as the expand view does, its boundary named as the unit names it.
    const target = { id: "@holon", type: "primitive", position: { x: 0, y: 0 }, data: { name: hname, body: { kind: "Composite", hash }, inputs: unit.inputs ?? [], outputs: unit.outputs ?? [], params: [], variadic: false, minInputs: 0 } } as PrimitiveNodeType;
    const opened = innerCanvas(target, [target], [], ev, catalog, variadic, nameOf);
    if (!opened.ok) throw new Error(`the holon "${hname}" can't be opened: ${opened.message}`);
    const inner = opened.nodes.filter((n) => n.id !== BOUNDARY_IN && n.id !== BOUNDARY_OUT) as PrimitiveNodeType[];
    const body = writeBody(inner, opened.edges, "  ");
    // The boundary, from the unit's own edges, by position (its names may repeat): each port
    // named uniquely in the text, the node's name for it where it has one, the unit's in quotes.
    const dIns = unique(unitIns.map((u, k) => display.inputs[k] ?? u));
    const dOuts = unique(unitOuts.map((u, k) => display.outputs[k] ?? u));
    const port = (shown: string, unitName: string) => (shown !== unitName ? `${qp(shown)} ${JSON.stringify(unitName)}` : qp(unitName));
    const child = (k: number) => opened.nodes.find((n) => n.id === opened.childIds[k]) as PrimitiveNodeType | undefined;
    const boundary: string[] = [];
    if (unit.body.kind === "Composite") {
      for (const e of unit.body.edges) {
        if (e.from_child === BOUNDARY) {
          const c = child(e.to_child);
          if (c) boundary.push(`  in.${qp(dIns[e.from_port])} -> ${body.names.get(c.id)}.${qp(c.data.inputs[e.to_port]?.name ?? "")}`);
        } else if (e.to_child === BOUNDARY) {
          const c = child(e.from_child);
          if (c) boundary.push(`  ${body.names.get(c.id)}.${qp(c.data.outputs[e.from_port]?.name ?? "")} -> out.${qp(dOuts[e.to_port])}`);
        }
      }
    }
    holonBlocks.push(
      [
        `holon ${hname}(${unitIns.map((u, k) => port(dIns[k], u)).join(", ")}) -> (${unitOuts.map((u, k) => port(dOuts[k], u)).join(", ")}) {`,
        ...body.lines,
        ...boundary,
        "}",
      ].join("\n"),
    );
    return hname;
  };

  /** Nodes and the wires between them, each node named for the text. */
  function writeBody(nodes: PrimitiveNodeType[], edges: Edge[], indent: string): { lines: string[]; names: Map<string, string> } {
    const names = new Map<string, string>();
    const taken = new Set<string>(["in", "out"]);
    const lines: string[] = [];
    for (const n of nodes) {
      const d = n.data;
      let kind: string;
      if (d.body.kind === "Composite") kind = holonFor(d.body.hash, { inputs: d.inputs.map((p) => p.name), outputs: d.outputs.map((p) => p.name) }, d.name);
      else kind = d.name;
      let base = ident(d.body.kind === "Composite" ? kind : d.name);
      let nname = base;
      for (let k = 2; taken.has(nname); k++) nname = `${base}${k}`;
      taken.add(nname);
      names.set(n.id, nname);
      // Params that differ from the leaf's or template's own; a literal's type and value always.
      const defaults = d.body.kind === "Template" ? templateDefaults(d.name as TemplateName) : d.body.kind === "Primitive" ? (catalog.get(d.name)?.params ?? []) : [];
      const params = d.params.filter((p) => {
        if (p.kind !== "Frozen") return false;
        if (d.name === "const" || d.name === "default") return true;
        const def = defaults.find((x) => x.name === p.name);
        return !def || def.kind !== "Frozen" || def.value !== p.value;
      });
      const shown = params.map((p) => `${q(p.name)}: ${q(p.kind === "Frozen" ? p.value : "")}`).join(", ");
      const declared = catalog.get(d.name)?.inputs?.length ?? 0;
      const arity = d.body.kind === "Primitive" && d.variadic && d.inputs.length > declared ? `[${d.inputs.length}]` : "";
      const pos = n.position ? ` at ${Math.round(n.position.x)} ${Math.round(n.position.y)}` : "";
      lines.push(`${indent}node ${nname} = ${q(kind)}${arity}${shown ? ` { ${shown} }` : ""}${pos}`);
    }
    for (const e of edges) {
      const a = names.get(e.source);
      const b = names.get(e.target);
      if (!a || !b) continue;
      lines.push(`${indent}${a}.${qp(e.sourceHandle ?? "")} -> ${b}.${qp(e.targetHandle ?? "")}`);
    }
    return { lines, names };
  }

  const top = writeBody(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], "");
  if (holonBlocks.length) out.push("", ...holonBlocks.flatMap((h) => [h, ""]));
  else out.push("");
  out.push(...top.lines);
  return out.join("\n").replace(/\n{3,}/g, "\n\n").trimEnd() + "\n";
}
