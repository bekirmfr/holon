// Deriving a server from the canvas: every connected group holding a `request` node is one route,
// with that node's method and path. Pure (types only from ./holon), so it is testable without the
// wasm package; App.tsx runs core's projection on each group in between these steps.

import type { WEncapSpec, WParam, WPort } from "./holon";
import { METHODS, parsePath, type Method } from "./endpoint";

export type CanvasNode = {
  id: string;
  data: { name: string; body: { kind: string }; params: WParam[]; outputs: WPort[]; inputs: WPort[] };
};
export type CanvasEdge = { source: string; target: string };

/** One route of the server: its request node, and every node wired (directly or not) to it. */
export type RouteGroup = { method: Method; path: string; requestId: string; nodeIds: string[]; resume: string };

export const isRequestNode = (n: CanvasNode) => n.data.name === "request" && n.data.body.kind === "Template";
export const isRespondNode = (n: CanvasNode) => n.data.name === "respond" && n.data.body.kind === "Template";

const frozen = (n: CanvasNode, name: string) => {
  const p = n.data.params.find((x) => x.name === name);
  return p && p.kind === "Frozen" ? p.value : "";
};

/** The canvas's connected groups (wires taken in either direction), each in canvas order. */
export function components(nodes: CanvasNode[], edges: CanvasEdge[]): string[][] {
  const parent = new Map(nodes.map((n) => [n.id, n.id]));
  const find = (x: string): string => {
    let r = x;
    while (parent.get(r) !== r) r = parent.get(r)!;
    parent.set(x, r);
    return r;
  };
  for (const e of edges) {
    if (parent.has(e.source) && parent.has(e.target)) parent.set(find(e.source), find(e.target));
  }
  const groups = new Map<string, string[]>();
  for (const n of nodes) {
    const root = find(n.id);
    groups.set(root, [...(groups.get(root) ?? []), n.id]);
  }
  return [...groups.values()];
}

/** The routes the canvas declares, and why any group can't be one. Groups without a request
 *  node are not part of the server (helpers, scratch work) and are skipped silently. */
export function findRoutes(nodes: CanvasNode[], edges: CanvasEdge[]): { routes: RouteGroup[]; errors: string[] } {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const routes: RouteGroup[] = [];
  const errors: string[] = [];
  for (const ids of components(nodes, edges)) {
    const requests = ids.map((id) => byId.get(id)!).filter(isRequestNode);
    if (requests.length === 0) continue;
    const label = (n: CanvasNode) => `${frozen(n, "method")} ${frozen(n, "path")}`;
    if (requests.length > 1) {
      errors.push(`${requests.map(label).join(" and ")} are wired together: one request node per route`);
      continue;
    }
    const req = requests[0];
    const method = frozen(req, "method");
    const path = frozen(req, "path");
    if (!(METHODS as readonly string[]).includes(method)) {
      errors.push(`"${method}" is not a method (${METHODS.join(", ")})`);
      continue;
    }
    if (parsePath(path) === null) {
      errors.push(`"${path}" is not a route path (use /todos or /todos/:id)`);
      continue;
    }
    // Two routes clash when they'd match the same requests, the rule core's read_server enforces.
    const shape = (p: string) => JSON.stringify(parsePath(p)!.map((s) => s.literal));
    const clash = routes.find((r) => r.method === method && shape(r.path) === shape(path));
    if (clash) {
      errors.push(`${method} ${path} clashes with ${clash.method} ${clash.path}`);
      continue;
    }
    routes.push({ method: method as Method, path, requestId: req.id, nodeIds: ids, resume: frozen(req, "resume") ?? "" });
  }
  return { routes, errors };
}

/**
 * Narrow core's proposed boundary for one route's group to what an endpoint needs. The only input
 * left open must be the request node's (the endpoint feeds it); the outputs kept are the respond
 * node's (the response) and every effect (state writes, which the host performs). Anything else
 * dangling is dropped from the endpoint, not an error: an unused `query` output is normal. An
 * input that may be left empty (an `http_request`'s `headers` or `body`) stays unbound, so it is
 * absent when the endpoint runs, rather than refusing the route.
 * `group` is in the same order as the children the spec was proposed over.
 */
export function routeSpec(spec: WEncapSpec, group: CanvasNode[], label: string): WEncapSpec | string {
  const promote = spec.promote.filter((p) => !isOptionalInput(group[p.child], group[p.child].data.inputs[p.port]?.name));
  for (const p of promote) {
    const n = group[p.child];
    if (!isRequestNode(n)) {
      return `${label}: ${n.data.name} · ${n.data.inputs[p.port]?.name ?? p.port} is not wired`;
    }
  }
  const expose = spec.expose.filter((x) => {
    const n = group[x.child];
    return isRespondNode(n) || n.data.outputs[x.port]?.ty === "Effect";
  });
  const responses = expose.filter((x) => isRespondNode(group[x.child]));
  if (responses.length > 1) return `${label}: more than one respond node`;
  if (expose.length === 0) return `${label}: add a respond node (or an effect, answered 204)`;
  // The response first: endpoint.ts takes the first non-effect output as output 0.
  return { promote, expose: [...responses, ...expose.filter((x) => !responses.includes(x))] };
}

/** Inputs a node works without: core treats them as empty when absent. */
function isOptionalInput(n: CanvasNode, port: string | undefined): boolean {
  return n.data.name === "http_request" && n.data.body.kind === "Primitive" && (port === "headers" || port === "body");
}
