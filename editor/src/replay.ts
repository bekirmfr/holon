// Replaying a logged run on the canvas, node by node. A flow log entry (holon-server's
// recorder.rs) holds everything the run depended on: the request record, the state it read, and
// the sourced values (clock, random, ingress). Evaluation is deterministic, so feeding those back
// in and evaluating each node in dependency order reproduces exactly what every node computed.
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
};

/** What one node computed: its inputs as they arrived, and its outputs. */
export type NodeTrace = { inputs: (WValue | null)[]; outputs: (WValue | null)[] };

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
    } else if (name === "request" && node.data.body.kind === "Template") {
      const fed = [entry.request];
      outputs = evalNode(node, fed);
      traces.set(id, { inputs: fed, outputs });
      continue;
    } else {
      outputs = evalNode(node, inputs);
    }
    traces.set(id, { inputs, outputs });
  }
  return { order, traces };
}
