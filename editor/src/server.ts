// A typed client for holon-server's REST API — the localhost transport that gives a browser-built
// flow somewhere real to live: staged (POST /unit), deployed (POST /deploy), triggered
// (POST /trigger/<slug>), and removed (DELETE /deployments/<slug>). Mirrors holon-server's actual
// JSON wire shapes exactly (crates/holon-server/src/main.rs) — the same "typed exactly against the
// real other side" discipline holon.ts already holds for the wasm bridge, now applied to this
// OTHER boundary the editor has.
//
// holon-server is on this machine by default (VITE_HOLON_SERVER names another), open there
// unless it was started with tokens (crates/holon-server/src/access.rs): then every request
// carries one, from what this browser was given under "access" (`api` below).

import type { WValue } from "./holon";
import type { EndpointDraft } from "./endpoint";
import type { NodeTiming } from "./replay";

const BASE: string = import.meta.env.VITE_HOLON_SERVER ?? "http://127.0.0.1:7420";

/** The tokens this browser sends the server: admin (every request) and secrets (writing a
 *  secret, when the server keeps that apart). Kept in this browser only. */
export type AccessTokens = { admin: string; secrets: string };
const TOKENS_KEY = "holon.access";

export function getAccessTokens(): AccessTokens {
  try {
    const t = JSON.parse(localStorage.getItem(TOKENS_KEY) ?? "{}") as Partial<AccessTokens>;
    return { admin: t.admin ?? "", secrets: t.secrets ?? "" };
  } catch {
    return { admin: "", secrets: "" };
  }
}

export function setAccessTokens(tokens: AccessTokens): void {
  try {
    if (!tokens.admin && !tokens.secrets) localStorage.removeItem(TOKENS_KEY);
    else localStorage.setItem(TOKENS_KEY, JSON.stringify(tokens));
  } catch {
    // storage unavailable (a private window): the tokens last for this page only
  }
}

/** The token a request carries: the secrets token to set, roll back or remove a secret when
 *  there is one, the admin token otherwise; none when neither is set. */
export function tokenFor(url: string, method: string, tokens: AccessTokens): string {
  const secretWrite = /\/deployments\/[^/]+\/secrets\/[^/?]+/.test(url) && (method === "POST" || method === "DELETE");
  return (secretWrite && tokens.secrets) || tokens.admin;
}

/** `fetch` against the server, carrying its token when one is set. */
async function api(url: string, init: RequestInit = {}): Promise<Response> {
  const token = tokenFor(url, (init.method ?? "GET").toUpperCase(), getAccessTokens());
  if (!token) return fetch(url, init);
  return fetch(url, { ...init, headers: { ...(init.headers as Record<string, string> | undefined), Authorization: `Bearer ${token}` } });
}

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
    // The server wants a token this browser hasn't been given: say where to give it.
    if (res.status === 401) message += " (set it under access, in the header)";
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
  const res = await api(`${BASE}/unit`, {
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
 *  unit's closure actually derives (via `Evaluator.capability_names`, computed by
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
  /** What it needs beyond its graph (limits, exceptions): part of its manifest. */
  requirements?: Requirements,
): Promise<DeployResult> {
  const res = await api(`${BASE}/deploy`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      slug,
      unit: unitHash,
      acknowledge,
      ...(intervalSeconds !== undefined ? { interval_seconds: intervalSeconds } : {}),
      ...(document !== undefined ? { document } : {}),
      ...(requirements !== undefined ? { requirements } : {}),
    }),
  });
  return asJson<DeployResult>(res);
}

/** What a flow declares it needs beyond its graph (permissions.rs's `Requirements`): limits set
 *  away from the host defaults, by name, and exceptions to the outbound rules. */
export type Requirements = {
  limits?: Record<string, number>;
  plain_http?: string[];
  private_addresses?: boolean;
  /** Capabilities it can run without: one not granted leaves it running with those calls
   *  refused, where a required one pauses it. Never its trigger (permissions.rs). */
  optional?: string[];
};

/** `POST /deploy/preview` — what deploying would ask for, without deploying. */
export type DeployPreview = {
  manifest: string;
  items: string[];
  /** What `/deploy` will want acknowledged. */
  needs_approval: string[];
  /** Granted already: by host policy, or by the deployment's current grant. */
  already_granted: string[];
  /** Why the host would refuse it, if it would. */
  refused: string | null;
  /** Secrets it reads that aren't set yet: those nodes stay empty until they are. */
  missing_secrets: string[];
};

/** A secret as the server lists it: never its value. `source`: `local` (sealed by the server),
 *  `env:HOLON_SECRET_…` or `vault:<mount>/<path>#<field>`. `problem`: why it has no value now. */
export type SecretInfo = { name: string; version: number; source: string; set_at: number; problem?: string };

/** `GET /deployments/<slug>/secrets` — the secrets set, never their values. */
export async function listSecrets(slug: string): Promise<SecretInfo[]> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/secrets`);
  return (await asJson<{ secrets: SecretInfo[] }>(res)).secrets;
}

const secretUrl = (slug: string, name: string) => `${BASE}/deployments/${encodeURIComponent(slug)}/secrets/${encodeURIComponent(name)}`;

/** Set a secret, as a new version: a value (sent to the server, sealed there, never sent back),
 *  or where it is kept (`from`: `env:…`, `vault:…`). Answers the new version. */
export async function setSecret(slug: string, name: string, set: { value: string } | { from: string }): Promise<number> {
  const res = await api(secretUrl(slug, name), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(set),
  });
  return (await asJson<{ version: number }>(res)).version;
}

/** A secret's kept versions (newest first) and its audit events (oldest first). */
export type SecretHistory = {
  versions: { version: number; source: string; set_at: number; current: boolean }[];
  /** `by`: who did it (a named user, or `admin` / `secrets` for the shared tokens), when known. */
  events: { at: number; name: string; action: string; version?: number; source?: string; detail?: string; by?: string }[];
};

/** One thing done on the server: a save, deploy, run, pause, stop, grant or delete, and by whom. */
export type ActivityEntry = { at: number; action: string; target: string; by?: string };

/** `GET /activity`: what was done, newest first. */
export async function getActivity(limit = 30): Promise<ActivityEntry[]> {
  return (await asJson<{ entries: ActivityEntry[] }>(await api(`${BASE}/activity?limit=${limit}`))).entries;
}

export async function getSecretHistory(slug: string, name: string): Promise<SecretHistory> {
  return asJson<SecretHistory>(await api(secretUrl(slug, name)));
}

/** Make `version` current again, as a new version; answers it. */
export async function rollbackSecret(slug: string, name: string, version: number): Promise<number> {
  const res = await api(`${secretUrl(slug, name)}/rollback`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ version }),
  });
  return (await asJson<{ version: number }>(res)).version;
}

export async function deleteSecret(slug: string, name: string): Promise<void> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/secrets/${encodeURIComponent(name)}`, { method: "DELETE" });
  if (!res.ok) await asJson(res);
}

export async function previewDeploy(slug: string, unitHash: string, requirements?: Requirements): Promise<DeployPreview> {
  const res = await api(`${BASE}/deploy/preview`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ slug, unit: unitHash, ...(requirements !== undefined ? { requirements } : {}) }),
  });
  return asJson<DeployPreview>(res);
}

/** `GET /permissions` — this host's policy. Limits by name: milliseconds, bytes, counts. */
export type HostPolicy = {
  defaults: Record<string, number>;
  ceilings: Record<string, number>;
  auto: string[];
  allow_plain_http: boolean;
  allow_private_addresses: boolean;
};

export async function getPermissions(): Promise<HostPolicy> {
  return asJson<HostPolicy>(await api(`${BASE}/permissions`));
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
  /** Who deployed it (a named user, or `admin`), when the server knows. */
  deployed_by?: string;
  /** The port a server deployment answers on; null for any other deployment. */
  port: number | null;
  status: DeploymentStatus;
  /** The current session's number (the last one's when stopped). */
  session: number;
  sessions: number;
  /** Why it can't run until deployed again (its grant no longer covers what it needs). */
  attention?: string;
  /** How many of its runs are waiting for an event. */
  waiting?: number;
};

/** A run waiting for an event (`GET /deployments/<slug>/runs`; never its tokens). */
export type WaitingRun = {
  run_id: string;
  kind: string;
  route: string | null;
  parked_at: number;
  waiting: { name: string; expires_at: number }[];
  delivered: string[];
};

/** `GET /deployments/<slug>/collections/<name>`: one item (`key`) or a page of items in key order
 *  (`prefix`, `after`, `limit`), as of `root` (one the deployment's log shows) or live. `next`:
 *  the key to continue after, when there may be more. */
export async function getCollectionItems(
  slug: string,
  name: string,
  query: { root?: string | null; key?: string; prefix?: string; after?: string | null; limit?: number },
): Promise<{ root: string; items: [string, WValue][]; next?: string | null }> {
  const q = new URLSearchParams();
  if (query.root) q.set("root", query.root);
  if (query.key !== undefined) q.set("key", query.key);
  if (query.prefix) q.set("prefix", query.prefix);
  if (query.after) q.set("after", query.after);
  if (query.limit) q.set("limit", String(query.limit));
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/collections/${encodeURIComponent(name)}?${q}`);
  return asJson(res);
}

export async function listWaitingRuns(slug: string): Promise<WaitingRun[]> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/runs`);
  return (await asJson<{ runs: WaitingRun[] }>(res)).runs;
}

/** What the host granted a deployment (permissions.rs's `Grant`): the items it holds, what it
 *  asked for beyond its graph, and what the host changed since the deploy. */
export type Grant = {
  manifest: string;
  items: string[];
  requirements: Requirements;
  granted_at: number;
  by: string;
  /** Items the host took back since. */
  revoked?: string[];
  /** Limits the host lowered since, by name. */
  lowered?: Record<string, number>;
  /** Whether it keeps running, the calls it lacks failing, when the grant doesn't cover it. */
  keep_running?: boolean;
  changes?: { at: number; change: string }[];
};

/** `GET`/`POST /deployments/<slug>/grant`: the grant, its hash, and why the deployment can't run
 *  under it, if it can't. */
export type GrantView = { grant: Grant; hash: string; attention: string | null };

/** `GET /deployments/<slug>/manifest`: what the deployment needs, re-derived from its unit, and
 *  what of it its grant doesn't cover. */
export type ManifestReply = {
  hash: string;
  capabilities: string[];
  requirements: Requirements;
  items: string[];
  /** The items it can run without, and what starts its runs (never optional). */
  optional: string[];
  trigger: string | null;
  not_granted: string[];
};

export async function getManifest(slug: string): Promise<ManifestReply> {
  return asJson<ManifestReply>(await api(`${BASE}/deployments/${encodeURIComponent(slug)}/manifest`));
}

export async function getGrant(slug: string): Promise<GrantView> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/grant`);
  return asJson<GrantView>(res);
}

/** Change what a deployment is granted, without a redeploy: revoke items, grant back items its
 *  manifest asks for, lower limits, or choose what an uncovered grant does. Whole or not at all. */
export async function changeGrant(
  slug: string,
  change: { revoke?: string[]; grant?: string[]; lower?: Record<string, number>; keep_running?: boolean },
): Promise<GrantView> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/grant`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(change),
  });
  return asJson<GrantView>(res);
}

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
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/${action}`, {
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
  /** The unit it ran (hex): the deployment's version then. Absent in entries from before. */
  unit?: string;
  session: number;
  at_ms: number;
  duration_ms: number;
  kind: "request" | "trigger" | "tick" | "resume";
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
  /** The outbound requests it made, round by round (absent when none). */
  rounds?: LoggedRound[];
  /** The collection reads it made, by node path, and what each answered (absent when none). */
  reads?: { path: number[]; request: WValue; answer: WValue | null }[];
  /** How many times it ran again because what it read changed before it could commit. */
  retries?: number;
  /** The run's id, the nonce in it, and the grant it was held to (hex; empty for no run). */
  run_id?: string;
  nonce?: string;
  grant?: string;
  /** The waiting points it settled waiting at (a durable run's part), if any. */
  parked?: string[];
  /** For a resumed part: the events delivered so far, by waiting point. */
  events?: Record<string, WValue>;
};

/** The requests one round made: each node (by its path in the run's root unit), its request,
 *  its response, and when, in microseconds from the run's start. */
export type LoggedRound = {
  exchanges: { path: number[]; request: WValue; response: WValue; start_us: number; end_us: number }[];
};

/** `GET /deployments/<slug>/log` — entries past `after` (all without), oldest first. `next` is
 *  the seq the next entry will get. */
export async function getLog(slug: string, after?: number): Promise<{ entries: LogEntry[]; next: number }> {
  const q = after === undefined ? "" : `?after=${after}`;
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/log${q}`);
  return asJson(res);
}

/** `GET /deployments/<slug>/document` — the canvas it was deployed from. Throws
 *  {@link ServerError} (404) for one deployed without it. */
/** The flow document a deployment was deployed from; with `unit`, that version's (the current
 *  one, or an earlier one it keeps), for a run logged on it. */
export async function getDeployedDocument(slug: string, unit?: string): Promise<FlowDocument> {
  const q = unit ? `?unit=${encodeURIComponent(unit)}` : "";
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/document${q}`);
  return asJson<FlowDocument>(res);
}

/** `GET /deployments/<slug>/sessions` — every run session, oldest first. */
export async function getSessions(slug: string): Promise<{ status: DeploymentStatus; sessions: SessionEntry[] }> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/sessions`);
  return asJson(res);
}

/** `GET /deployments` — every currently deployed flow. */
export async function listDeployments(): Promise<DeploymentEntry[]> {
  const res = await api(`${BASE}/deployments`);
  const { deployments } = await asJson<{ deployments: DeploymentEntry[] }>(res);
  return deployments;
}

/** `DELETE /deployments/<slug>` — stop a deployment. Sufficient on its own: the scheduler only
 *  ever acts on what's currently in the table, and an ingress-triggered slug starts 404ing on
 *  `/trigger` immediately afterward. */
export async function deleteDeployment(slug: string): Promise<DeploymentEntry> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}`, { method: "DELETE" });
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
  const res = await api(`${BASE}/trigger/${encodeURIComponent(slug)}`, {
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
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/request`, {
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
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/state`);
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
  /** What its composites are called, by hash (docUnits.ts): a unit keeps no name, so a holon
   *  inside another would otherwise show as its hash once reopened. Absent in older flows. */
  names?: Record<string, string>;
  /** The flow's deploy settings (DeployBar): the port for a flow with routes, the interval for a
   *  clock flow. Absent in older flows; `endpoints`, from flows saved before routes were
   *  derived, is ignored. */
  server?: { port: string; interval?: string; endpoints?: EndpointDraft[]; requirements?: Requirements };
  /** For a deployed flow: which canvas node each step of a run is, by route (`"*"` for a flow
   *  deployed whole), so the log's timings land on the canvas (replay.ts's `nodeTimings`). */
  trace?: Record<string, string[]>;
};

/** One entry in `GET /flows` — name + freshness only, never the (possibly large) document. */
/** `saved_by`: who saved it (a named user, or `admin`), when the server knows. */
export type FlowListEntry = { name: string; saved_at_epoch_seconds: number; saved_by?: string };

/** A full saved flow as `GET /flows/<name>` returns it. */
export type FlowRecord = FlowListEntry & { document: FlowDocument };

/** `POST /flows` — save (upsert) the current canvas under `name`. The same name saves over
 *  the previous version: a flow is a mutable workspace file, and "save" means exactly what it
 *  means in any editor. Throws {@link ServerError} (400) for an invalid name. */
export async function saveFlow(name: string, document: FlowDocument): Promise<FlowListEntry> {
  const res = await api(`${BASE}/flows`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, document }),
  });
  return asJson<FlowListEntry>(res);
}

/** `GET /flows` — every saved flow's name and freshness. */
export async function listFlows(): Promise<FlowListEntry[]> {
  const res = await api(`${BASE}/flows`);
  return asJson<FlowListEntry[]>(res);
}

/** `GET /flows/<name>` — one full saved flow, document included. Throws (404) if unknown. */
export async function getFlow(name: string): Promise<FlowRecord> {
  const res = await api(`${BASE}/flows/${encodeURIComponent(name)}`);
  return asJson<FlowRecord>(res);
}

/** Every saved flow, documents included (to see which flows place which as holons). */
export async function getAllFlows(): Promise<FlowRecord[]> {
  const list = await listFlows();
  return Promise.all(list.map((f) => getFlow(f.name)));
}

/** `DELETE /flows/<name>` — remove a saved flow. Throws (404) if it was never there. */
export async function deleteFlow(name: string): Promise<void> {
  const res = await api(`${BASE}/flows/${encodeURIComponent(name)}`, { method: "DELETE" });
  await asJson<{ deleted: string }>(res);
}