// Replaying a logged run on the canvas, node by node. A flow log entry (holon-server's
// recorder.rs) holds everything the run depended on: the request record, the state it read, and
// the sourced values (clock, random, ingress). Evaluation is deterministic, so feeding those back
// in and evaluating each node in dependency order reproduces exactly what every node computed.
// Except where a secret or a token went in: the log never holds those (a placeholder stands in),
// so a node fed one is shown with what the run recorded it produced, not recomputed. The same
// for a holon (a placed composite) that reads from outside inside it, a clock, a secret, state,
// the network: the replay has none of those to give it, so it shows what the run recorded.
//
// Which nodes run follows core: from the run's answer and effects back through what each needs,
// a choice (`guard`, `coalesce`) needing only what core's choice rule (through wasm) says, so a
// branch the run didn't take shows as not run, not as run with nothing.
//
// Evaluation itself is passed in (`evalNode`), so this is testable without a real evaluator.

import { choiceNext, type WParam, type WPort, type WValue } from "./holon";
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
  /** The nodes of the run's group it didn't run, each with the choice that skipped it (`null`
   *  when none did: a cycle, or nothing the run needed uses it). */
  notRun: Map<string, string | null>;
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
  run: Pick<Replay, "order" | "traces">,
  nodes: ReplayNode[],
  step: number | null,
): { after: Record<string, WValue>; changed: string[] } {
  const upTo = step === null ? run.order.length - 1 : step;
  const after = { ...before };
  const changed = new Set<string>();
  // A write is a `store_write` effect any node put out: a store_write's own, or one coming out of
  // a holon that writes inside it.
  run.order.slice(0, upTo + 1).forEach((id) => {
    if (!nodes.some((x) => x.id === id)) return;
    for (const out of run.traces.get(id)?.outputs ?? []) {
      if (out?.type !== "Effect" || !out.value.sink.startsWith("store_write:")) continue;
      const key = out.value.sink.slice(out.value.sink.indexOf(":") + 1);
      after[key] = out.value.payload;
      if (JSON.stringify(before[key] ?? null) !== JSON.stringify(out.value.payload)) changed.add(key);
    }
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
  run: Pick<Replay, "order" | "traces"> | null,
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
  // Puts and deletes are the effects any node put out: a collection node's own, or a holon's.
  const effects = (run?.order.slice(0, upTo + 1) ?? [])
    .filter((id) => nodes.some((x) => x.id === id))
    .flatMap((id) => run!.traces.get(id)?.outputs ?? []);
  for (const out of effects) {
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

/** One step inside a canvas node (a holon's own node, or a template's) as a run recorded it. */
export type InnerStep = { child: number; outputs: (WValue | null)[] | null; start_us: number; end_us: number };

/**
 * What the nodes inside canvas node `nodeId` did in a run, one level down (the log keeps that
 * level: holon-server's Timer::DEPTH), from its final round, in the order they started. `child` is
 * the index into the node's own unit's children. Empty when the log doesn't go that deep (runs
 * logged before it did) or the trace can't place the node.
 */
export function innerSteps(
  entry: { kind: string; route: string | null; timeline?: NodeTiming[] },
  trace: Record<string, string[]> | undefined,
  nodeId: string,
): InnerStep[] {
  const map = trace?.[entry.kind === "request" ? (entry.route ?? "") : "*"];
  const k = map ? map.indexOf(nodeId) : -1;
  if (k < 0) return [];
  const prefix = entry.kind === "request" ? [0, k] : [k];
  const last = Math.max(0, ...(entry.timeline ?? []).map((t) => t.round ?? 0));
  return (entry.timeline ?? [])
    .filter((t) => (t.round ?? 0) === last && t.path.length === prefix.length + 1 && prefix.every((p, i) => t.path[i] === p))
    .sort((a, b) => a.start_us - b.start_us)
    .map((t) => ({ child: t.path[prefix.length], outputs: t.outputs ? t.outputs.map((o) => o ?? null) : null, start_us: t.start_us, end_us: t.end_us }));
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
 * Re-run `entry` over the canvas, as core runs it: from the nodes the run's answer and effects
 * come from (and any node nothing consumes), back through what each needs. An ordinary node needs
 * every wired input; a choice (`guard`, `coalesce`) only what core's choice rule says, so a branch
 * not taken isn't run, here as in the run. Sourced leaves take their recorded values; a request
 * node takes the recorded request; every other node is evaluated by `evalNode` on the values its
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
  /** Whether a node reads from outside within itself (a holon holding a clock, a secret, state,
   *  a request…): shown with what the run recorded it produced, as it can't be recomputed. */
  readsOutside: (node: ReplayNode) => boolean = () => false,
): Replay {
  const ids = new Set(runGroup(nodes, edges, entry));
  const group = nodes.filter((n) => ids.has(n.id));
  const byId = new Map(group.map((n) => [n.id, n]));
  const wires = edges.filter((e) => ids.has(e.source) && ids.has(e.target));
  const traces = new Map<string, NodeTrace>();
  const order: string[] = [];
  const notRun = new Map<string, string | null>();

  // A cycle can't be deployed; if a canvas has one anyway, nothing runs, as in core.
  const indegree = new Map(group.map((n) => [n.id, 0]));
  for (const e of wires) indegree.set(e.target, (indegree.get(e.target) ?? 0) + 1);
  const ready = group.filter((n) => indegree.get(n.id) === 0).map((n) => n.id);
  let sorted = 0;
  while (ready.length > 0) {
    const id = ready.pop()!;
    sorted++;
    for (const e of wires.filter((w) => w.source === id)) {
      indegree.set(e.target, indegree.get(e.target)! - 1);
      if (indegree.get(e.target) === 0) ready.push(e.target);
    }
  }
  if (sorted < group.length) {
    for (const n of group) notRun.set(n.id, null);
    return { order, traces, notRun };
  }

  // The roots, as the unit the run ran has them: for a request, the respond node, every effect
  // left unwired, and any node nothing consumes; for a whole flow, every node with an output left
  // unwired. Among them, the one the run evaluated first goes first, then canvas order.
  const wiredOut = (n: ReplayNode, port: string) => wires.some((w) => w.source === n.id && w.sourceHandle === port);
  const isRoot = (n: ReplayNode) =>
    entry.kind === "request"
      ? !wires.some((w) => w.source === n.id) ||
        (n.data.name === "respond" && n.data.body.kind === "Template") ||
        n.data.outputs.some((p) => p.ty === "Effect" && !wiredOut(n, p.name))
      : n.data.outputs.length === 0 || n.data.outputs.some((p) => !wiredOut(n, p.name));
  const rank = (id: string) => {
    const i = executed.indexOf(id);
    return [i < 0 ? Infinity : i, group.findIndex((n) => n.id === id)];
  };
  const before = (a: string, b: string) => {
    const [ra, ca] = rank(a);
    const [rb, cb] = rank(b);
    return ra !== rb ? ra - rb : ca - cb;
  };
  const roots = group
    .filter(isRoot)
    .map((n) => n.id)
    .sort(before);

  const sourced = new Map(entry.sourced);
  // An absent output can cross the wasm boundary as undefined: absent is null here, always.
  const evaluate = (node: ReplayNode, inputs: (WValue | null)[]) => evalNode(node, inputs).map((o) => o ?? null);
  const incoming = (id: string, port: string) => wires.find((w) => w.target === id && w.targetHandle === port);
  const carried = (w: ReplayEdge) => {
    const at = byId.get(w.source)!.data.outputs.findIndex((p) => p.name === w.sourceHandle);
    return traces.get(w.source)?.outputs[at] ?? null;
  };
  // Each input as seen so far: undefined while the node feeding it hasn't been evaluated.
  const seenBy = (node: ReplayNode) =>
    node.data.inputs.map((port) => {
      const wire = incoming(node.id, port.name);
      if (!wire) return null;
      return traces.has(wire.source) ? carried(wire) : undefined;
    });

  // What one node computes once everything it needs has arrived.
  const run = (id: string, node: ReplayNode, inputs: (WValue | null)[]): NodeTrace => {
    const name = node.data.name;
    const primitive = node.data.body.kind === "Primitive";
    const param = (p: string) => frozen(node.data.params, p) ?? "";
    if (primitive && name === "store_read") return { inputs, outputs: [entry.state_before[param("key")] ?? null] };
    if (primitive && name === "clock") return { inputs, outputs: [sourced.get("Clock") ?? null] };
    if (primitive && name === "random") return { inputs, outputs: [sourced.get("Random") ?? null] };
    if (primitive && name === "ingress") return { inputs, outputs: [sourced.get("NetworkIngress") ?? null] };
    // A waiting point answers with the event the log says was delivered, if any.
    if (primitive && name === "await") return { inputs, outputs: [entry.events?.[param("name")] ?? null] };
    // A token is a credential: the log has it redacted, and so does the replay.
    if (primitive && name === "resume_token") return { inputs, outputs: [{ type: "Text", value: `‹token:${param("await")}›` }] };
    // The editor never has a secret's value: a placeholder stands in, as the log shows it.
    if (primitive && name === "secret") return { inputs, outputs: [{ type: "Text", value: `‹secret:${param("name")}›` }] };
    if (primitive && (name === "http_request" || name === "collection_get" || name === "collection_list")) {
      // Answered from the record, never the network or the store: a replay is offline and
      // exact, and shows the collection as the run found it, whatever it holds now.
      return { inputs, outputs: [responses.get(id) ?? null] };
    }
    if (name === "request" && node.data.body.kind === "Template") {
      const fed = [entry.request];
      return { inputs: fed, outputs: evaluate(node, fed) };
    }
    if (inputs.some(hasPlaceholder) || readsOutside(node)) {
      // Fed a secret or token the log doesn't hold, or reading from outside within itself: what
      // the run recorded, if it did.
      const recorded = produced.get(id);
      return recorded ? { inputs, outputs: recorded, recorded: true } : { inputs, outputs: evaluate(node, inputs), placeholder: true };
    }
    return { inputs, outputs: evaluate(node, inputs) };
  };

  // Depth first from the roots, with an explicit stack, as core walks: a node runs once what it
  // needs is evaluated; until then, the node feeding what it needs next is pushed.
  const stack = [...roots].reverse();
  while (stack.length > 0) {
    const id = stack[stack.length - 1];
    if (traces.has(id)) {
      stack.pop();
      continue;
    }
    const node = byId.get(id)!;
    const seen = seenBy(node);
    const choice = node.data.body.kind === "Primitive" ? choiceNext(node.data.name, seen) : null;
    const need = choice ? ("need" in choice ? choice.need : -1) : seen.findIndex((v) => v === undefined);
    if (need >= 0) {
      stack.push(incoming(id, node.data.inputs[need].name)!.source);
      continue;
    }
    stack.pop();
    order.push(id);
    const inputs = seen.map((v) => v ?? null);
    traces.set(id, choice && "done" in choice ? { inputs, outputs: [choice.done] } : run(id, node, inputs));
  }

  // A node not run was skipped by the first choice on its way to the roots: the nearest node
  // downstream that ran (only a choice runs without everything it is fed).
  for (const n of group) {
    if (traces.has(n.id)) continue;
    const visited = new Set([n.id]);
    const queue = [n.id];
    let by: string | null = null;
    while (queue.length > 0 && by === null) {
      const at = queue.shift()!;
      for (const w of wires.filter((x) => x.source === at)) {
        if (traces.has(w.target)) {
          by = w.target;
          break;
        }
        if (!visited.has(w.target)) {
          visited.add(w.target);
          queue.push(w.target);
        }
      }
    }
    notRun.set(n.id, by);
  }
  return { order, traces, notRun };
}

/** Why a node didn't run, in a line: the choice that skipped it (`by`, from `notRun`), in its
 *  own terms. */
export function whyNotRun(nodes: ReplayNode[], by: string | null): string {
  const choice = by === null ? undefined : nodes.find((n) => n.id === by);
  if (!choice) return "not run: nothing the run needed uses it";
  switch (choice.data.name) {
    case "guard":
      return "not run: the guard after it didn't take this branch";
    case "coalesce":
      return "not run: the coalesce after it already had a value";
    default:
      return `not run: the ${choice.data.name} after it didn't need it`;
  }
}

/** Whether the replay ran the nodes the run ran, where the run's timeline (its final round) says.
 *  `null` when it can't tell: no timeline, a run that waited (what waits on an event isn't run,
 *  but a replay has nothing to wait for), or one logged before choices were lazy (its timeline
 *  lists every node, taken or not). */
export function sameNodesRan(group: string[], run: Replay, timings: Map<string, unknown>, waited: boolean): boolean | null {
  if (timings.size === 0 || waited) return null;
  if (run.notRun.size > 0 && group.every((id) => timings.has(id))) return null;
  const ran = new Set(run.order);
  return group.every((id) => ran.has(id) === timings.has(id));
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
