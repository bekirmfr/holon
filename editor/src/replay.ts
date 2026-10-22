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
): Replay {
  const ids = new Set(runGroup(nodes, edges, entry));
  const group = nodes.filter((n) => ids.has(n.id));
  const wires = edges.filter((e) => ids.has(e.source) && ids.has(e.target));

  // Dependency order (Kahn), ties broken by canvas order, so a replay is itself deterministic.
  const indegree = new Map(group.map((n) => [n.id, 0]));
  for (const e of wires) indegree.set(e.target, (indegree.get(e.target) ?? 0) + 1);
  const ready = group.filter((n) => indegree.get(n.id) === 0).map((n) => n.id);
  const order: string[] = [];
  while (ready.length > 0) {
    const id = ready.shift()!;
    order.push(id);
    for (const e of wires.filter((w) => w.source === id)) {
      const left = indegree.get(e.target)! - 1;
      indegree.set(e.target, left);
      if (left === 0) {
        ready.push(e.target);
        ready.sort((a, b) => group.findIndex((n) => n.id === a) - group.findIndex((n) => n.id === b));
      }
    }
  }

  const sourced = new Map(entry.sourced);
  const traces = new Map<string, NodeTrace>();
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
    } else if (node.data.body.kind === "Primitive" && name === "http_request") {
      // Answered from the record, never the network: a replay is offline and exact.
      outputs = [responses.get(id) ?? null];
    } else if (name === "request" && node.data.body.kind === "Template") {
      const fed = [entry.request];
      outputs = evalNode(node, fed);
      traces.set(id, { inputs: fed, outputs });
      continue;
    } else if (inputs.some(hasPlaceholder)) {
      // Fed a secret or token the log doesn't hold: what the run recorded, if it did.
      const recorded = produced.get(id);
      if (recorded) {
        traces.set(id, { inputs, outputs: recorded, recorded: true });
      } else {
        traces.set(id, { inputs, outputs: evalNode(node, inputs), placeholder: true });
      }
      continue;
    } else {
      outputs = evalNode(node, inputs);
    }
    traces.set(id, { inputs, outputs });
  }
  return { order, traces };
}
