// Replaying a logged run on the canvas, node by node. A flow log entry (holon-server's
// recorder.rs) holds everything the run depended on: the request record, the state it read, and
// the sourced values (clock, random, ingress). Evaluation is deterministic, so feeding those back
// in and evaluating each node in dependency order reproduces exactly what every node computed.
// Except where a secret or a token went in: the log never holds those (a placeholder stands in),
// so a node fed one is shown with what the run recorded it produced, not recomputed.
//
// Pure: evaluation itself is passed in (`evalNode`), so this is testable without the wasm
// package. Only TYPES come from ./holon.

import type { WParam, WPort, WValue } from "./holon";
import { components } from "./derive";

export type ReplayNode = {
  id: string;
  data: { name: string; body: { kind: string }; params: WParam[]; inputs: WPort[]; outputs: WPort[] };
};
export type ReplayEdge = { source: string; target: string; sourceHandle?: string | null; targetHandle?: string | null };

/** The parts of a log entry a replay needs (server.ts's LogEntry has more). */
export type ReplayInput = {
  kind: string;
  route: string | null;
  request: WValue | null;
  sourced: [string, WValue][];
  state_before: Record<string, WValue>;
  /** A resumed part's delivered events, by waiting point. */
  events?: Record<string, WValue>;
};

/** What one node computed: its inputs as they arrived, and its outputs. `recorded`: the outputs
 *  are the run's own, from its log, because an input carried a secret or token placeholder.
 *  `placeholder`: such a node the log has no record for, recomputed from the placeholder, so its
 *  outputs may differ from the run's. */
export type NodeTrace = {
  inputs: (WValue | null)[];
  outputs: (WValue | null)[];
  recorded?: boolean;
  placeholder?: boolean;
  /** Its part in a failed run, once diagnosed (`diagnose`). */
  issue?: Issue;
};

/** Whether `v` carries a secret's or token's placeholder anywhere (the log's redaction). */
export function hasPlaceholder(v: WValue | null): boolean {
  if (v === null) return false;
  switch (v.type) {
    case "Text":
      return v.value.includes("‹secret:") || v.value.includes("‹token:");
    case "List":
      return v.value.some(hasPlaceholder);
    case "Record":
      return v.value.some(([, x]) => hasPlaceholder(x));
    case "Effect":
      return hasPlaceholder(v.value.payload);
    case "Sum":
      return hasPlaceholder(v.value.payload);
    default:
      return false;
  }
}

export type Replay = {
  /** The nodes the run went through, in the order they were evaluated. */
  order: string[];
  traces: Map<string, NodeTrace>;
};

const frozen = (params: WParam[], name: string) => {
  const p = params.find((x) => x.name === name);
  return p && p.kind === "Frozen" ? p.value : undefined;
};

/**
 * The flow's state as of a step: what the run read at its start, and what it holds once the
 * state writes evaluated so far (`store_write` nodes up to and including `step`; all of them for
 * `null`) are applied. The host commits writes when the run finishes, so "after" at the last step
 * is the state the next run sees.
 */
export function stateAtStep(
  before: Record<string, WValue>,
  run: Replay,
  nodes: ReplayNode[],
  step: number | null,
): { after: Record<string, WValue>; changed: string[] } {
  const upTo = step === null ? run.order.length - 1 : step;
  const after = { ...before };
  const changed = new Set<string>();
  run.order.slice(0, upTo + 1).forEach((id) => {
    const n = nodes.find((x) => x.id === id);
    if (!n || n.data.name !== "store_write" || n.data.body.kind !== "Primitive") return;
    const out = run.traces.get(id)?.outputs[0];
    if (out?.type !== "Effect") return;
    const key = out.value.sink.slice(out.value.sink.indexOf(":") + 1);
    after[key] = out.value.payload;
    if (JSON.stringify(before[key] ?? null) !== JSON.stringify(out.value.payload)) changed.add(key);
  });
  return { after, changed: [...changed].sort() };
}

/** One item of a collection a run touched. `before`: what it held when the run started, as the
 *  run read it (`null`: no such item); `undefined` when the run didn't read it, so the log
 *  doesn't say. `after`: what the steps so far wrote (`null`: deleted); `undefined` when
 *  unwritten. */
export type ItemChange = { key: string; before?: WValue | null; after?: WValue | null; read: boolean };

/** A collection as a run saw and changed it: its root when the run started (a state cell under
 *  `#collection:<name>`, the hash of its content), the items it read or wrote, and its listings. */
export type CollectionAtStep = {
  name: string;
  root: string | null;
  items: ItemChange[];
  listed: { prefix: string; after: string | null; count: number }[];
};

const COLLECTION_CELL = "#collection:";

/** Whether state key `k` is a collection's root (shown as its collection, not as a cell). */
export const isCollectionCell = (k: string) => k.startsWith(COLLECTION_CELL);

/**
 * The collections a run touched, as of `step` (all of it for `null`): the items it read, with
 * what each held, and the puts and deletes evaluated so far. Built from the log (its reads, its
 * state before) and the replay (each `collection_put`/`collection_delete` node's effect), the
 * way `stateAtStep` builds a state cell's change from `store_write`.
 */
export function collectionsAtStep(
  entry: { state_before: Record<string, WValue>; reads?: { request: WValue; answer: WValue | null }[] },
  run: Replay | null,
  nodes: ReplayNode[],
  step: number | null,
): CollectionAtStep[] {
  const byName = new Map<string, CollectionAtStep>();
  const of = (name: string) => {
    let c = byName.get(name);
    if (!c) {
      const cell = entry.state_before[COLLECTION_CELL + name];
      c = { name, root: cell?.type === "Text" ? cell.value : null, items: [], listed: [] };
      byName.set(name, c);
    }
    return c;
  };
  const item = (c: CollectionAtStep, key: string) => {
    let i = c.items.find((x) => x.key === key);
    if (!i) {
      i = { key, read: false };
      c.items.push(i);
    }
    return i;
  };
  const field = (v: WValue | null | undefined, k: string) => (v?.type === "Record" ? (v.value.find(([x]) => x === k)?.[1] ?? null) : null);
  const text = (v: WValue | null) => (v?.type === "Text" ? v.value : null);

  for (const k of Object.keys(entry.state_before)) if (isCollectionCell(k)) of(k.slice(COLLECTION_CELL.length));
  for (const r of entry.reads ?? []) {
    const name = text(field(r.request, "collection"));
    if (!name) continue;
    const key = text(field(r.request, "get"));
    if (key !== null) {
      const i = item(of(name), key);
      i.before = r.answer;
      i.read = true;
      continue;
    }
    const list = field(r.request, "list");
    if (list) {
      const count = r.answer?.type === "Record" ? r.answer.value.length : 0;
      of(name).listed.push({ prefix: text(field(list, "prefix")) ?? "", after: text(field(list, "after")), count });
    }
  }
  const upTo = step === null ? (run?.order.length ?? 0) - 1 : step;
  for (const id of run?.order.slice(0, upTo + 1) ?? []) {
    const n = nodes.find((x) => x.id === id);
    if (!n || n.data.body.kind !== "Primitive" || !n.data.name.startsWith("collection_")) continue;
    const out = run!.traces.get(id)?.outputs[0];
    if (out?.type !== "Effect") continue;
    const [sink, name] = [out.value.sink.split(":")[0], out.value.sink.slice(out.value.sink.indexOf(":") + 1)];
    if (sink === "collection_put") {
      const key = text(field(out.value.payload, "key"));
      if (key !== null) item(of(name), key).after = field(out.value.payload, "value");
    } else if (sink === "collection_delete") {
      const key = text(out.value.payload);
      if (key !== null) item(of(name), key).after = null;
    }
  }
  for (const c of byName.values()) c.items.sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
  return [...byName.values()].sort((a, b) => (a.name < b.name ? -1 : 1));
}

/** Whether a run changed an item: it wrote it, and (when the log says what was there) wrote
 *  something else. A write over an item it never read counts: the log can't tell otherwise. */
export const itemChanged = (i: ItemChange) =>
  i.after !== undefined && (i.before === undefined || JSON.stringify(i.after) !== JSON.stringify(i.before));

/** When one node ran, in microseconds from the run's start, and in which round of the run
 *  (holon-server's NodeTiming; `round` absent from servers before rounds). */
export type NodeTiming = {
  path: number[];
  start_us: number;
  end_us: number;
  round?: number;
  /** What the node produced (servers before this, or an output too large, leave it out). */
  outputs?: (WValue | null)[];
};

/** A run's step paths, as the server records them, against canvas node ids. `trace` comes with
 *  the deployed flow: for each route (or `"*"` for a whole-flow unit), which canvas node is each
 *  child of the unit as stored (canonical order). A request's steps sit one level down, inside its
 *  endpoint (`[0, k]`); a whole-flow unit's at the top (`[k]`). */
function nodeAtPath(
  entry: { kind: string; route: string | null },
  trace: Record<string, string[]> | undefined,
): (path: number[]) => string | undefined {
  const map = trace?.[entry.kind === "request" ? (entry.route ?? "") : "*"];
  const depth = entry.kind === "request" ? 2 : 1;
  return (path) => {
    if (!map || path.length !== depth || (depth === 2 && path[0] !== 0)) return undefined;
    return map[path[depth - 1]];
  };
}

/** One outbound request a run made, by the canvas node that made it. */
export type NodeExchange = {
  id: string;
  round: number;
  request: WValue;
  response: WValue;
  start_us: number;
  end_us: number;
};

/** The run's outbound requests, round by round, each placed on its canvas node (one the trace
 *  can't place is left out). */
export function nodeExchanges(
  entry: { kind: string; route: string | null; rounds?: { exchanges: { path: number[]; request: WValue; response: WValue; start_us: number; end_us: number }[] }[] },
  trace: Record<string, string[]> | undefined,
): NodeExchange[] {
  const at = nodeAtPath(entry, trace);
  return (entry.rounds ?? []).flatMap((r, round) =>
    r.exchanges.flatMap((x) => {
      const id = at(x.path);
      return id ? [{ id, round, request: x.request, response: x.response, start_us: x.start_us, end_us: x.end_us }] : [];
    }),
  );
}

/** What each `http_request` node was answered, from the run's record: the last answer it got,
 *  which is the one its final round used. Replay feeds these back instead of calling anything. */
export function recordedResponses(exchanges: NodeExchange[]): Map<string, WValue> {
  const out = new Map<string, WValue>();
  for (const x of exchanges) out.set(x.id, x.response);
  return out;
}

/** What each collection read (`collection_get`, `collection_list`) was answered, by canvas
 *  node, from the run's record: replay answers them from here, as the run found the collection.
 *  A read that found nothing is left out, and replays as nothing. */
export function recordedReads(
  entry: { kind: string; route: string | null; reads?: { path: number[]; answer: WValue | null }[] },
  trace: Record<string, string[]> | undefined,
): Map<string, WValue> {
  const out = new Map<string, WValue>();
  const at = nodeAtPath(entry, trace);
  for (const r of entry.reads ?? []) {
    const id = at(r.path);
    if (id && r.answer !== null) out.set(id, r.answer);
  }
  return out;
}

/**
 * The server's per-step timings, by canvas node. `trace` comes with the deployed flow: for each
 * route (or `"*"` for a whole-flow unit), which canvas node is each child of the unit as stored
 * (canonical order). A request's steps sit one level down, inside its endpoint (`[0, k]`); a
 * whole-flow unit's at the top (`[k]`).
 */
export function nodeTimings(
  entry: { kind: string; route: string | null; timeline?: NodeTiming[] },
  trace: Record<string, string[]> | undefined,
): Map<string, { start_us: number; end_us: number }> {
  const out = new Map<string, { start_us: number; end_us: number }>();
  const at = nodeAtPath(entry, trace);
  // The final round is the one whose outputs and effects counted; earlier rounds ran with
  // answers still missing.
  const last = Math.max(0, ...(entry.timeline ?? []).map((t) => t.round ?? 0));
  for (const t of entry.timeline ?? []) {
    if ((t.round ?? 0) !== last) continue;
    const id = at(t.path);
    if (id) out.set(id, { start_us: t.start_us, end_us: t.end_us });
  }
  return out;
}

/** The canvas nodes in the order the run evaluated them (its final round's start times). */
export function executionOrder(timings: Map<string, { start_us: number; end_us: number }>): string[] {
  return [...timings.entries()].sort(([, a], [, b]) => a.start_us - b.start_us).map(([id]) => id);
}

/** What each node produced in the run's final round, by canvas node, where the log has it. */
export function nodeOutputs(
  entry: { kind: string; route: string | null; timeline?: NodeTiming[] },
  trace: Record<string, string[]> | undefined,
): Map<string, (WValue | null)[]> {
  const out = new Map<string, (WValue | null)[]>();
  const at = nodeAtPath(entry, trace);
  const last = Math.max(0, ...(entry.timeline ?? []).map((t) => t.round ?? 0));
  for (const t of entry.timeline ?? []) {
    if ((t.round ?? 0) !== last || !t.outputs) continue;
    const id = at(t.path);
    if (id) out.set(id, t.outputs.map((o) => o ?? null));
  }
  return out;
}

/**
 * Which nodes a run touched: for a request, the group holding the request node of its route
 * (`"GET /todos/:id"`); for anything else (a tick, a trigger), the whole canvas.
 */
export function runGroup(nodes: ReplayNode[], edges: ReplayEdge[], entry: ReplayInput): string[] {
  if (entry.kind !== "request" || !entry.route) return nodes.map((n) => n.id);
  const [method, path] = [entry.route.slice(0, entry.route.indexOf(" ")), entry.route.slice(entry.route.indexOf(" ") + 1)];
  const req = nodes.find(
    (n) =>
      n.data.name === "request" &&
      n.data.body.kind === "Template" &&
      frozen(n.data.params, "method") === method &&
      frozen(n.data.params, "path") === path,
  );
  if (!req) return [];
  return components(nodes, edges).find((ids) => ids.includes(req.id)) ?? [];
}

/**
 * Re-run `entry` over the canvas. Sourced leaves take their recorded values; a request node
 * takes the recorded request; every other node is evaluated by `evalNode` on the values its
 * wires bring in (`null` for an unwired or absent input).
 */
export function replay(
  nodes: ReplayNode[],
  edges: ReplayEdge[],
  entry: ReplayInput,
  evalNode: (node: ReplayNode, inputs: (WValue | null)[]) => (WValue | null)[],
  /** What each `http_request` node was answered in the run (`recordedResponses`). */
  responses: Map<string, WValue> = new Map(),
  /** What each node produced in the run (`nodeOutputs`): used for a node fed a placeholder. */
  produced: Map<string, (WValue | null)[]> = new Map(),
  /** The order the run evaluated its nodes in (`executionOrder`), followed where it applies. */
  executed: string[] = [],
): Replay {
  const ids = new Set(runGroup(nodes, edges, entry));
  const group = nodes.filter((n) => ids.has(n.id));
  const wires = edges.filter((e) => ids.has(e.source) && ids.has(e.target));

  // Dependency order (Kahn). Among the nodes ready to run, the one the run itself evaluated first
  // goes first, then canvas order: so a replay steps through the nodes as the run did (core
  // evaluates in a dependency order too), and is deterministic without timings.
  const rank = (id: string) => {
    const i = executed.indexOf(id);
    return [i < 0 ? Infinity : i, group.findIndex((n) => n.id === id)];
  };
  const before = (a: string, b: string) => {
    const [ra, ca] = rank(a);
    const [rb, cb] = rank(b);
    return ra !== rb ? ra - rb : ca - cb;
  };
  const indegree = new Map(group.map((n) => [n.id, 0]));
  for (const e of wires) indegree.set(e.target, (indegree.get(e.target) ?? 0) + 1);
  const ready = group.filter((n) => indegree.get(n.id) === 0).map((n) => n.id);
  ready.sort(before);
  const order: string[] = [];
  while (ready.length > 0) {
    const id = ready.shift()!;
    order.push(id);
    for (const e of wires.filter((w) => w.source === id)) {
      const left = indegree.get(e.target)! - 1;
      indegree.set(e.target, left);
      if (left === 0) {
        ready.push(e.target);
        ready.sort(before);
      }
    }
  }

  const sourced = new Map(entry.sourced);
  const traces = new Map<string, NodeTrace>();
  // An absent output can cross the wasm boundary as undefined: absent is null here, always.
  const evaluate = (node: ReplayNode, inputs: (WValue | null)[]) => evalNode(node, inputs).map((o) => o ?? null);
  for (const id of order) {
    const node = group.find((n) => n.id === id)!;
    const inputs = node.data.inputs.map((port) => {
      const wire = wires.find((w) => w.target === id && w.targetHandle === port.name);
      if (!wire) return null;
      const from = group.find((n) => n.id === wire.source)!;
      const at = from.data.outputs.findIndex((p) => p.name === wire.sourceHandle);
      return traces.get(wire.source)?.outputs[at] ?? null;
    });
    const name = node.data.name;
    let outputs: (WValue | null)[];
    if (node.data.body.kind === "Primitive" && name === "store_read") {
      outputs = [entry.state_before[frozen(node.data.params, "key") ?? ""] ?? null];
    } else if (node.data.body.kind === "Primitive" && name === "clock") {
      outputs = [sourced.get("Clock") ?? null];
    } else if (node.data.body.kind === "Primitive" && name === "random") {
      outputs = [sourced.get("Random") ?? null];
    } else if (node.data.body.kind === "Primitive" && name === "ingress") {
      outputs = [sourced.get("NetworkIngress") ?? null];
    } else if (node.data.body.kind === "Primitive" && name === "await") {
      // A waiting point answers with the event the log says was delivered, if any.
      outputs = [entry.events?.[frozen(node.data.params, "name") ?? ""] ?? null];
    } else if (node.data.body.kind === "Primitive" && name === "resume_token") {
      // A token is a credential: the log has it redacted, and so does the replay.
      outputs = [{ type: "Text", value: `‹token:${frozen(node.data.params, "await") ?? ""}›` }];
    } else if (node.data.body.kind === "Primitive" && name === "secret") {
      // The editor never has a secret's value: a placeholder stands in, as the log shows it.
      outputs = [{ type: "Text", value: `‹secret:${frozen(node.data.params, "name") ?? ""}›` }];
    } else if (node.data.body.kind === "Primitive" && (name === "http_request" || name === "collection_get" || name === "collection_list")) {
      // Answered from the record, never the network or the store: a replay is offline and
      // exact, and shows the collection as the run found it, whatever it holds now.
      outputs = [responses.get(id) ?? null];
    } else if (name === "request" && node.data.body.kind === "Template") {
      const fed = [entry.request];
      outputs = evaluate(node, fed);
      traces.set(id, { inputs: fed, outputs });
      continue;
    } else if (inputs.some(hasPlaceholder)) {
      // Fed a secret or token the log doesn't hold: what the run recorded, if it did.
      const recorded = produced.get(id);
      if (recorded) {
        traces.set(id, { inputs, outputs: recorded, recorded: true });
      } else {
        traces.set(id, { inputs, outputs: evaluate(node, inputs), placeholder: true });
      }
      continue;
    } else {
      outputs = evaluate(node, inputs);
    }
    traces.set(id, { inputs, outputs });
  }
  return { order, traces };
}

// ── Why a run failed ──────────────────────────────────────────────────────────────────────────
// A value that never arrives is how a flow fails: a node gets everything it was wired and still
// produces nothing (a field the record doesn't have, a signature that doesn't verify), and every
// node after it is starved of that value, down to the answer (a `respond` falling back to its
// `missing` status) or the effect that never happens. The diagnosis finds that node, the origin,
// says why in its own terms, and marks the path the absence took.

/** A wire, as one key: which output feeds which input. */
export const wireKey = (w: ReplayEdge) => `${w.source}:${w.sourceHandle ?? ""}→${w.target}:${w.targetHandle ?? ""}`;

/** A node's part in a failed run. `origin`: it got all its inputs and produced nothing (or its
 *  outbound call failed): where the failure starts. `starved`: an input never arrived, so it
 *  didn't run, or fell back (a `respond`'s `missing` status). */
export type Issue = { role: "origin" | "starved"; reason: string };

export type Diagnosis = {
  /** What went wrong, in a line: the answer, or the run's error. */
  summary: string;
  /** The origins, in evaluation order. */
  origins: string[];
  issues: Map<string, Issue>;
  /** The wires the failure travelled along (`wireKey`), from each origin downstream. */
  path: Set<string>;
};

const keysOf = (v: WValue | null) => (v?.type === "Record" ? v.value.map(([k]) => k) : null);
const textOf = (v: WValue | null) => (v?.type === "Text" ? v.value : null);
const fieldOf = (v: WValue | null, key: string) => (v?.type === "Record" ? (v.value.find(([k]) => k === key)?.[1] ?? null) : null);
const listOf = (keys: string[]) => (keys.length === 0 ? "nothing" : keys.slice(0, 8).join(", ") + (keys.length > 8 ? ", …" : ""));

/** Why `node` produced nothing from the inputs it got, as precisely as its kind allows. */
function whyNothing(node: ReplayNode, trace: NodeTrace): string {
  const p = (name: string) => frozen(node.data.params, name);
  const [a, b] = trace.inputs;
  switch (node.data.name) {
    case "field": {
      const keys = keysOf(a);
      return keys ? `no field "${p("name")}" (it has ${listOf(keys)})` : "its input isn't a record";
    }
    case "lookup": {
      const keys = keysOf(a);
      return keys ? `no key "${textOf(b) ?? "?"}" (it has ${listOf(keys)})` : "its input isn't a record";
    }
    case "collection_get":
      return `no item "${textOf(a) ?? "?"}" in the collection "${p("collection")}"`;
    case "find":
      return `no item whose "${p("name")}" matches`;
    case "get":
      return "no item at that index";
    case "head":
    case "tail":
      return "the list is empty";
    case "guard":
      return "its condition was false";
    case "json_parse":
      return "its text isn't valid JSON";
    case "divide":
      return "division by zero";
    case "stripe_verify":
      return trace.outputs[1]?.type === "Boolean"
        ? "the signature or its timestamp didn't verify"
        : "no stripe-signature header, or one it can't read";
    case "respond":
      return "nothing to answer with";
    case "request":
      return "the request's body isn't valid JSON";
    default:
      return "it got its inputs but produced nothing (a value of the wrong kind, or one it can't use)";
  }
}

/** Why an outbound call's answer counts as a failure, if it does. */
function whyFailed(response: WValue | null): string | null {
  const status = fieldOf(response, "status");
  if (status?.type !== "Number") return null;
  if (status.value === 0) {
    const error = textOf(fieldOf(response, "error")) ?? "failed";
    const message = textOf(fieldOf(response, "message"));
    return `the request failed: ${error}${message ? ` (${message})` : ""}`;
  }
  return status.value >= 400 ? `the other side answered ${status.value}` : null;
}

/**
 * Why `entry` failed, traced over its replay: `null` when it didn't (no error, a status under
 * 400). Every node that got all its wired inputs and still left a wired output empty, or whose
 * outbound call failed, is an origin; every node missing a wired input is starved; the path runs
 * from each origin along the wires that carried nothing (and the origin's own wires out).
 */
export function diagnose(
  nodes: ReplayNode[],
  edges: ReplayEdge[],
  entry: { status: number | null; error: string | null },
  run: Replay,
): Diagnosis | null {
  const failed = entry.error !== null || (entry.status !== null && entry.status >= 400);
  if (!failed) return null;
  const inRun = new Set(run.order);
  const wires = edges.filter((w) => inRun.has(w.source) && inRun.has(w.target));
  const node = (id: string) => nodes.find((n) => n.id === id)!;
  const carried = (w: ReplayEdge) => {
    const from = node(w.source);
    const at = from.data.outputs.findIndex((p) => p.name === w.sourceHandle);
    return run.traces.get(w.source)?.outputs[at] ?? null;
  };

  const issues = new Map<string, Issue>();
  const origins: string[] = [];
  for (const id of run.order) {
    const n = node(id);
    const trace = run.traces.get(id);
    if (!trace) continue;
    const fedBy = wires.filter((w) => w.target === id);
    const missing = fedBy.filter((w) => carried(w) === null);
    if (missing.length > 0) {
      const from = missing.map((w) => `${w.targetHandle} (from ${node(w.source).data.name})`).join(", ");
      const fellBack = trace.outputs.some((o) => o !== null);
      issues.set(id, { role: "starved", reason: `${fellBack ? "fell back: " : "didn't run: "}${from} never arrived` });
      continue;
    }
    if (n.data.name === "http_request" && n.data.body.kind === "Primitive") {
      const why = whyFailed(trace.outputs[0] ?? null);
      if (why) {
        issues.set(id, { role: "origin", reason: why });
        origins.push(id);
        continue;
      }
    }
    const feeds = wires.filter((w) => w.source === id);
    if (feeds.some((w) => carried(w) === null)) {
      issues.set(id, { role: "origin", reason: whyNothing(n, trace) });
      origins.push(id);
    }
  }

  // The path: from each origin, its own wires out, then every wire that carried nothing.
  const path = new Set<string>();
  const reached = new Set(origins);
  const queue = [...origins];
  while (queue.length > 0) {
    const id = queue.shift()!;
    for (const w of wires.filter((x) => x.source === id)) {
      if (!origins.includes(id) && carried(w) !== null) continue;
      path.add(wireKey(w));
      if (!reached.has(w.target)) {
        reached.add(w.target);
        queue.push(w.target);
      }
    }
  }
  // Starved nodes the absence never reached from an origin (fed by an unwired, absent input) keep
  // their mark but stay off the path.
  const summary = entry.error ? `the run failed: ${entry.error}` : `answered ${entry.status}`;
  return { summary, origins, issues, path };
}
