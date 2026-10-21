// A typed client for holon-server's REST API — the localhost transport that gives a browser-built
// flow somewhere real to live: staged (POST /unit), deployed (POST /deploy), triggered
// (POST /trigger/<slug>), and removed (DELETE /deployments/<slug>). Mirrors holon-server's actual
// JSON wire shapes exactly (crates/holon-server/src/main.rs) — the same "typed exactly against the
// real other side" discipline holon.ts already holds for the wasm bridge, now applied to this
// OTHER boundary the editor has.
//
// holon-server is localhost-only, no auth (content-addressing already secures correctness — see
// its own transport-scoping L4). Every function here assumes it's running at this fixed origin.

import type { WValue } from "./holon";
import type { EndpointDraft } from "./endpoint";
import type { NodeTiming } from "./replay";

const BASE = "http://127.0.0.1:7420";

/** holon-server's own error shape: `{"error": "..."}`, returned alongside a non-2xx status. */
type ServerErrorBody = { error: string };

/** Thrown by every function below on a non-2xx response — carries the real HTTP status and
 *  holon-server's own message, not a generic "request failed." A 404 with an empty body (how
 *  `Response::empty_404()` actually replies — confirmed against the real server, not assumed)
 *  falls back to the browser's own status text rather than fail trying to parse JSON that was
 *  never there. */
export class ServerError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
    this.name = "ServerError";
  }
}

async function asJson<T>(res: Response): Promise<T> {
  if (!res.ok) {
    let message = res.statusText;
    try {
      const body = (await res.json()) as ServerErrorBody;
      if (body?.error) message = body.error;
    } catch {
      // No JSON body (an empty 404, or something unexpected) — the status text above is the
      // honest fallback, not a second, unrelated parse error.
    }
    throw new ServerError(res.status, message);
  }
  return res.json() as Promise<T>;
}

/** `POST /unit` — stage a unit's canonical preimage bytes, returning its hash (hex). The SAME
 *  hash `Evaluator.insert` already computed locally (one content-addressing scheme) — staging
 *  gives the local identity somewhere to live server-side too, it doesn't mint a new one.
 *
 *  `preimage`'s real static type is `Uint8Array<ArrayBufferLike>` (TypeScript's newer, generic
 *  `Uint8Array`) — `ArrayBufferLike` includes `SharedArrayBuffer`, which `BlobPart`/`BodyInit`
 *  correctly refuse (a `Blob` needs to read bytes once and know they can't change under it, a
 *  guarantee a *shared*, mutable buffer can't give). Nothing here ever creates a
 *  `SharedArrayBuffer` — `preimage` always comes fresh from `build_preimage()` — so this is a
 *  real gap in what the *type* can prove, not a real risk in what the *value* is. Rather than
 *  cast past that gap, build a plain `ArrayBuffer` — a type that's never ambiguous, never
 *  generic — and copy the bytes into it explicitly. Correct by construction, not by trust; and
 *  byte-safe regardless of whether `preimage` is ever a sub-view of a larger buffer, since only
 *  its own `byteLength` is copied. */
export async function stageUnit(preimage: Uint8Array): Promise<string> {
  const buffer = new ArrayBuffer(preimage.byteLength);
  new Uint8Array(buffer).set(preimage);
  const res = await fetch(`${BASE}/unit`, {
    method: "POST",
    headers: { "Content-Type": "application/octet-stream" },
    body: buffer,
  });
  const { hash } = await asJson<{ hash: string }>(res);
  return hash;
}

/** `POST /deploy`'s response — mirrors `DeployResponseBody` exactly. `triggers` is what the
 *  server actually *derived* from the unit's own capabilities, not an echo of what was asked
 *  for — the same "derived, not asserted" property the server itself enforces. */
export type DeployResult = {
  slug: string;
  unit: string;
  triggers: string[];
  interval_seconds: number | null;
};

/** `POST /deploy` — register `slug -> unit`. `acknowledge` must name every capability the
 *  unit's closure actually derives (via `capabilityAcknowledgeName` from `./holon`, computed by
 *  the caller from `Evaluator.capabilities()`) — the server re-derives the real set independently
 *  and checks it, never trusts this list as the truth. `intervalSeconds` only matters — and is
 *  only required, server-side — when the derived capabilities include `Clock`. */
export async function deployUnit(
  slug: string,
  unitHash: string,
  acknowledge: string[],
  intervalSeconds?: number,
  /** The flow's canvas, kept with the deployment so view mode shows exactly what runs. */
  document?: FlowDocument,
): Promise<DeployResult> {
  const res = await fetch(`${BASE}/deploy`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      slug,
      unit: unitHash,
      acknowledge,
      ...(intervalSeconds !== undefined ? { interval_seconds: intervalSeconds } : {}),
      ...(document !== undefined ? { document } : {}),
    }),
  });
  return asJson<DeployResult>(res);
}

/** One entry from `GET /deployments` (or `DELETE /deployments/<slug>`'s response) — mirrors
 *  `DeploymentEntry` exactly, including the state/lifetime fields that came with real
 *  per-deployment state. */
export type DeploymentEntry = {
  slug: string;
  unit: string;
  triggers: string[];
  interval_seconds: number | null;
  has_state: boolean;
  trigger_count: number;
  error_count: number;
  deployed_at_epoch_seconds: number;
  /** The port a server deployment answers on; null for any other deployment. */
  port: number | null;
  status: DeploymentStatus;
  /** The current session's number (the last one's when stopped). */
  session: number;
  sessions: number;
};

/** Running answers and fires; paused keeps its port and session but answers 503; stopped has
 *  ended its session (saving its state) and released its port. Mirrors lifecycle.rs. */
export type DeploymentStatus = "running" | "paused" | "stopped";

/** One run session of a deployment (lifecycle.rs's `Session`). `state` is the state when it
 *  ended: null while it runs, and for a deployment without state. */
export type SessionEntry = {
  number: number;
  started_at: number;
  ended_at: number | null;
  requests: number;
  errors: number;
  /** "deploy", "latest", "fresh" or "session N": what it started from. */
  from: string;
  state: Record<string, WValue> | null;
};

/** `POST /deployments/<slug>/run|pause|stop`. `from` (run after a stop only): "latest", "fresh",
 *  or a session number whose saved state to start from. Throws {@link ServerError} (409) when
 *  the move doesn't apply, e.g. pausing a stopped deployment. */
export async function changeDeployment(
  slug: string,
  action: "run" | "pause" | "stop",
  from?: "latest" | "fresh" | number,
): Promise<DeploymentEntry> {
  const res = await fetch(`${BASE}/deployments/${encodeURIComponent(slug)}/${action}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(from === undefined ? {} : { from }),
  });
  return asJson<DeploymentEntry>(res);
}

/** One run in a deployment's flow log (holon-server's recorder.rs): what it read and produced,
 *  enough to replay it exactly (replay.ts). */
export type LogEntry = {
  seq: number;
  session: number;
  at_ms: number;
  duration_ms: number;
  kind: "request" | "trigger" | "tick";
  /** The matched route, e.g. "GET /todos/:id". */
  route: string | null;
  method: string | null;
  url: string | null;
  status: number | null;
  request: WValue | null;
  /** By capability: "Clock", "Random", "NetworkIngress". */
  sourced: [string, WValue][];
  state_before: Record<string, WValue>;
  outputs: (WValue | null)[];
  performed: PerformedEntry[];
  response_body: string | null;
  error: string | null;
  /** When each step ran (absent from servers before it was recorded). */
  timeline?: NodeTiming[];
};

/** `GET /deployments/<slug>/log` — entries past `after` (all without), oldest first. `next` is
 *  the seq the next entry will get. */
export async function getLog(slug: string, after?: number): Promise<{ entries: LogEntry[]; next: number }> {
  const q = after === undefined ? "" : `?after=${after}`;
  const res = await fetch(`${BASE}/deployments/${encodeURIComponent(slug)}/log${q}`);
  return asJson(res);
}

/** `GET /deployments/<slug>/document` — the canvas it was deployed from. Throws
 *  {@link ServerError} (404) for one deployed without it. */
export async function getDeployedDocument(slug: string): Promise<FlowDocument> {
  const res = await fetch(`${BASE}/deployments/${encodeURIComponent(slug)}/document`);
  return asJson<FlowDocument>(res);
}

/** `GET /deployments/<slug>/sessions` — every run session, oldest first. */
export async function getSessions(slug: string): Promise<{ status: DeploymentStatus; sessions: SessionEntry[] }> {
  const res = await fetch(`${BASE}/deployments/${encodeURIComponent(slug)}/sessions`);
  return asJson(res);
}

/** `GET /deployments` — every currently deployed flow. */
export async function listDeployments(): Promise<DeploymentEntry[]> {
  const res = await fetch(`${BASE}/deployments`);
  const { deployments } = await asJson<{ deployments: DeploymentEntry[] }>(res);
  return deployments;
}

/** `DELETE /deployments/<slug>` — stop a deployment. Sufficient on its own: the scheduler only
 *  ever acts on what's currently in the table, and an ingress-triggered slug starts 404ing on
 *  `/trigger` immediately afterward. */
export async function deleteDeployment(slug: string): Promise<DeploymentEntry> {
  const res = await fetch(`${BASE}/deployments/${encodeURIComponent(slug)}`, { method: "DELETE" });
  return asJson<DeploymentEntry>(res);
}

/** One `(capability, payload)` pair a trigger actually performed — mirrors `PerformedEntry`. */
export type PerformedEntry = { capability: string; payload: WValue };

/** `POST /trigger/<slug>`'s response — mirrors `FireResponseBody` exactly, `outputs` included:
 *  every settled slot the fired unit declared, not just one. A single-output unit (the common
 *  case) is still a one-element array; a flow that produces a genuine data response *and*
 *  performs a genuine side effect can now be left as two separate exposed outputs — no more
 *  forcing them into one artificial `list(...)` just to satisfy an old single-output rule.
 *  `SValue` (the server's own JSON mirror of `core::Value`) and `WValue` (the wasm bridge's)
 *  turned out to be the identical `{type,value}` shape, independently — so `WValue` is reused
 *  here rather than a second, parallel value type for this other boundary. */
export type TriggerResult = {
  outputs: (WValue | null)[];
  performed: PerformedEntry[];
};

/** `POST /trigger/<slug>` — fire an ingress-triggered deployment with a real request body.
 *  Throws {@link ServerError} (404) if the slug isn't deployed. */
export async function triggerDeployment(slug: string, body: string): Promise<TriggerResult> {
  // A string body would default to text/plain, which the server refuses (a CORS-safelisted type
  // any site could send without preflight); the ingress text goes as raw bytes instead.
  const res = await fetch(`${BASE}/trigger/${encodeURIComponent(slug)}`, {
    method: "POST",
    headers: { "Content-Type": "application/octet-stream" },
    body,
  });
  return asJson<TriggerResult>(res);
}

/** One HTTP request for a deployed server, and its answer (`POST /deployments/<slug>/request`). */
export type HttpRequest = { method: string; url: string; headers: [string, string][]; body: string };
export type HttpResponse = { status: number; headers: [string, string][]; body: string };

/** Send `request` to the server deployed as `slug`, through holon-server's control plane rather
 *  than the server's own port (which the browser can't read from: no CORS there). It is routed,
 *  fired and counted exactly as a request on the port would be. Throws {@link ServerError} (404)
 *  for an unknown slug, (400) for a deployment that isn't a server. */
export async function sendRequest(slug: string, request: HttpRequest): Promise<HttpResponse> {
  const res = await fetch(`${BASE}/deployments/${encodeURIComponent(slug)}/request`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(request),
  });
  return asJson<HttpResponse>(res);
}

/** `GET /deployments/<slug>/state` — a stateful deployment's current keyed state. Throws
 *  {@link ServerError} — 400 if the slug exists but was never stateful, 404 if it doesn't
 *  exist at all; two different failures, two different statuses, not one glossed-over "empty". */
export async function getDeploymentState(slug: string): Promise<Record<string, WValue>> {
  const res = await fetch(`${BASE}/deployments/${encodeURIComponent(slug)}/state`);
  const { state } = await asJson<{ slug: string; state: Record<string, WValue> }>(res);
  return state;
}

// ── flows: named workspace documents (P4.8: save / open / delete) ──────────────────────────

/** What a saved flow document carries — the canvas exactly as it was left. This shape is the
 *  EDITOR's own contract with itself: the server stores it opaquely (validated only as JSON,
 *  named by the same slug rule deployments use) and never reads inside it, so this type lives
 *  here, not mirrored from any server struct. `nodes`/`edges` are React Flow's own arrays as
 *  the canvas holds them (positions included); `viewport` is the pan/zoom triple its
 *  `getViewport()` returns. `unknown[]` rather than the precise React Flow node type on
 *  purpose — server.ts stays free of React Flow imports (it's the plain HTTP boundary), and
 *  App.tsx, which owns both sides of this contract, casts at the seam. */
export type FlowDocument = {
  nodes: unknown[];
  edges: unknown[];
  viewport: { x: number; y: number; zoom: number };
  /** Hex preimages of every unit the canvas's composite nodes reference (their whole closure),
   *  inserted back into the Evaluator on open. Absent in flows saved before it existed. */
  units?: string[];
  /** The flow's deploy settings (DeployBar): the port for a flow with routes, the interval for a
   *  clock flow. Absent in older flows; `endpoints`, from flows saved before routes were
   *  derived, is ignored. */
  server?: { port: string; interval?: string; endpoints?: EndpointDraft[] };
  /** For a deployed flow: which canvas node each step of a run is, by route (`"*"` for a flow
   *  deployed whole), so the log's timings land on the canvas (replay.ts's `nodeTimings`). */
  trace?: Record<string, string[]>;
};

/** One entry in `GET /flows` — name + freshness only, never the (possibly large) document. */
export type FlowListEntry = { name: string; saved_at_epoch_seconds: number };

/** A full saved flow as `GET /flows/<name>` returns it. */
export type FlowRecord = FlowListEntry & { document: FlowDocument };

/** `POST /flows` — save (upsert) the current canvas under `name`. The same name saves over
 *  the previous version: a flow is a mutable workspace file, and "save" means exactly what it
 *  means in any editor. Throws {@link ServerError} (400) for an invalid name. */
export async function saveFlow(name: string, document: FlowDocument): Promise<FlowListEntry> {
  const res = await fetch(`${BASE}/flows`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, document }),
  });
  return asJson<FlowListEntry>(res);
}

/** `GET /flows` — every saved flow's name and freshness. */
export async function listFlows(): Promise<FlowListEntry[]> {
  const res = await fetch(`${BASE}/flows`);
  return asJson<FlowListEntry[]>(res);
}

/** `GET /flows/<name>` — one full saved flow, document included. Throws (404) if unknown. */
export async function getFlow(name: string): Promise<FlowRecord> {
  const res = await fetch(`${BASE}/flows/${encodeURIComponent(name)}`);
  return asJson<FlowRecord>(res);
}

/** `DELETE /flows/<name>` — remove a saved flow. Throws (404) if it was never there. */
export async function deleteFlow(name: string): Promise<void> {
  const res = await fetch(`${BASE}/flows/${encodeURIComponent(name)}`, { method: "DELETE" });
  await asJson<{ deleted: string }>(res);
}