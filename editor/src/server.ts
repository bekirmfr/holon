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
): Promise<DeployResult> {
  const res = await fetch(`${BASE}/deploy`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      slug,
      unit: unitHash,
      acknowledge,
      ...(intervalSeconds !== undefined ? { interval_seconds: intervalSeconds } : {}),
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
};

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

/** `POST /trigger/<slug>`'s response — mirrors `FireResponseBody` exactly. `SValue` (the
 *  server's own JSON mirror of `core::Value`) and `WValue` (the wasm bridge's) turned out to be
 *  the identical `{type,value}` shape, independently — so `WValue` is reused here rather than a
 *  second, parallel value type for this other boundary. */
export type TriggerResult = {
  output: WValue | null;
  performed: PerformedEntry[];
};

/** `POST /trigger/<slug>` — fire an ingress-triggered deployment with a real request body.
 *  Throws {@link ServerError} (404) if the slug isn't deployed. */
export async function triggerDeployment(slug: string, body: string): Promise<TriggerResult> {
  const res = await fetch(`${BASE}/trigger/${encodeURIComponent(slug)}`, { method: "POST", body });
  return asJson<TriggerResult>(res);
}

/** `GET /deployments/<slug>/state` — a stateful deployment's current keyed state. Throws
 *  {@link ServerError} — 400 if the slug exists but was never stateful, 404 if it doesn't
 *  exist at all; two different failures, two different statuses, not one glossed-over "empty". */
export async function getDeploymentState(slug: string): Promise<Record<string, WValue>> {
  const res = await fetch(`${BASE}/deployments/${encodeURIComponent(slug)}/state`);
  const { state } = await asJson<{ slug: string; state: Record<string, WValue> }>(res);
  return state;
}