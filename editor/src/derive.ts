// The routes a flow declares: every connected group holding a `request` node is one route, with
// that node's method and path. Decided by holon-author (crates/holon-author/src/routes.rs),
// through the wasm bridge: the same code the command line runs.

import { canvas_components, find_routes } from "../wasm/holon_wasm.js";
import type { Method } from "./endpoint";

/** One route of the server: its request node, and every node wired (directly or not) to it. */
export type RouteGroup = { method: Method; path: string; requestId: string; nodeIds: string[]; resume: string };

/** The canvas's connected groups of node ids (wires taken in either direction), in canvas order. */
export function components(nodes: { id: string }[], edges: { source: string; target: string }[]): string[][] {
  return canvas_components(
    nodes.map((n) => n.id),
    edges.map((e) => ({ source: e.source, target: e.target })),
  );
}

/** The routes the canvas declares, and why any group can't be one. Groups without a request
 *  node are not part of the server (helpers, scratch work) and are skipped silently. */
export function findRoutes(nodes: unknown[], edges: unknown[]): { routes: RouteGroup[]; errors: string[] } {
  return find_routes(nodes, edges);
}
