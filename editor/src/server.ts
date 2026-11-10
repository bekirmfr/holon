// A typed client for the Holon server's REST API (`holon serve`, crates/holon): the transport that
// gives a browser-built flow somewhere real to live: staged (POST /unit), deployed (POST /deploy),
// triggered (POST /trigger/<slug>), and removed (DELETE /deployments/<slug>). Mirrors the server's
// actual JSON wire shapes exactly (its handlers in crates/holon/src) — the same "typed exactly
// against the real other side" discipline holon.ts already holds for the wasm bridge, applied to
// this OTHER boundary the editor has.
//
// The server is the one this editor was served from, or 127.0.0.1:7420 in development
// (VITE_HOLON_SERVER names another). Everyone signs in (crates/holon/src/access.rs): every request
// carries the session this browser was given, a bearer token, never a cookie (AuthGate.tsx).

import { browsersOf } from "./DeployBar";
import { signedIn, signOut, type AuthConfig } from "./signin";
import { usesIn, type Placed } from "./holonUsage";
import type { WValue, ModuleEntry } from "./holon";
import type { EndpointDraft } from "./endpoint";
import type { NodeTiming } from "./replay";
import type { SettingChange, Touch } from "./touches";

/** The server: VITE_HOLON_SERVER when set; else, built (`npm run build`), the server it is served
 *  from (holon-server serves the editor itself, editor.rs), and in development the local one. */
const BASE: string = import.meta.env.VITE_HOLON_SERVER ?? (import.meta.env.DEV ? "http://127.0.0.1:7420" : "");

/** A role on the server: what an account or token may do (access.rs). */
export type Role = "viewer" | "editor" | "admin" | "agent";
/** Who this browser is signed in as: the session token the server gave, and who it is. */
export type Session = { token: string; name: string; role: Role };
const SESSION_KEY = "holon.session";
let memorySession: Session | null = null;

export function getSession(): Session | null {
  try {
    return (JSON.parse(localStorage.getItem(SESSION_KEY) ?? "null") as Session | null) ?? memorySession;
  } catch {
    return memorySession;
  }
}

/** Keep (or, with null, forget) this browser's session; who listens is told. */
export function setSession(session: Session | null): void {
  memorySession = session;
  try {
    if (session) localStorage.setItem(SESSION_KEY, JSON.stringify(session));
    else localStorage.removeItem(SESSION_KEY);
  } catch {
    // storage unavailable: signed in for this page only
  }
  window.dispatchEvent(new Event("holon:session"));
}

/** `fetch` against the server, carrying who is signed in: the session's token, else the ID
 *  token of a sign-in through the server's identity provider (signin.ts). A 401 means the
 *  sign-in has ended: it is forgotten, and the editor asks to sign in again. */
async function api(url: string, init: RequestInit = {}): Promise<Response> {
  const token = getSession()?.token ?? signedIn()?.idToken;
  const res = await fetch(url, token ? { ...init, headers: { ...(init.headers as Record<string, string> | undefined), Authorization: `Bearer ${token}` } } : init);
  if (res.status === 401 && token) {
    setSession(null);
    signOut();
  }
  return res;
}

const post = (url: string, body?: unknown) =>
  api(url, { method: "POST", headers: { "Content-Type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });

/** `GET /version`: the version the host runs, for the host card. */
export async function getVersion(): Promise<string> {
  const res = await api(`${BASE}/version`);
  if (!res.ok) throw new Error(`${res.status}`);
  return (await res.text()).trim();
}

/** Where the editor's server is, as the host card says it: its address, or this page's. */
export const hostAddress = (): string => (BASE ? new URL(BASE).host : window.location.host);

/** `GET /auth/state`: whether the server waits for its first admin, and where to sign in
 *  through a provider, when it has one. */
export async function getAuthState(): Promise<{ setup: boolean; oidc: AuthConfig | null }> {
  return asJson(await fetch(`${BASE}/auth/state`));
}

type SignedInAs = { token: string; name: string; role: Role };
const keep = async (res: Response) => {
  const s = await asJson<SignedInAs>(res);
  setSession(s);
  return s;
};
/** The first admin, with the code the server printed; signed in. */
export const setUp = (code: string, name: string, password: string) => post(`${BASE}/auth/setup`, { code, name, password }).then(keep);
export const signInWith = (name: string, password: string) => post(`${BASE}/auth/sign-in`, { name, password }).then(keep);
/** Whose invite link `token` is. */
export const inviteFor = async (token: string) => asJson<{ name: string }>(await fetch(`${BASE}/auth/invite?token=${encodeURIComponent(token)}`));
export const acceptInvite = (token: string, password: string) => post(`${BASE}/auth/invite`, { token, password }).then(keep);
export const whoAmI = async () => asJson<{ name: string; role: Role; email?: string }>(await api(`${BASE}/auth/me`));
export async function changePassword(current: string, next: string): Promise<void> {
  await asJson<void>(await post(`${BASE}/auth/password`, { current, new: next }));
}
export async function signOutHere(): Promise<void> {
  await post(`${BASE}/auth/sign-out`).catch(() => null);
  setSession(null);
  signOut();
}

/** What the editor keeps for whoever is signed in (home.ts's Prefs, as the server holds it). */
export const getPrefs = async () => asJson<unknown>(await api(`${BASE}/auth/prefs`));
/** Saves go one at a time, in the order made: several in quick succession (a pane added, moved,
 *  removed) could otherwise land out of order, leaving an older one on the server. */
let prefsSaved: Promise<unknown> = Promise.resolve();
export function setPrefs(prefs: unknown): Promise<void> {
  const save = prefsSaved.then(async () => asJson<void>(await post(`${BASE}/auth/prefs`, prefs)));
  prefsSaved = save.catch(() => undefined);
  return save;
}

// ── the host's pages ──────────────────────────────────────────────────────────────────────
export type AccountEntry = { name: string; role: Role; email: string | null; disabled: boolean; created_at: number; created_by: string | null; invited: boolean };
export type TokenEntry = { id: string; label: string; role: Role; created_at: number; created_by: string };
export type OidcSettings = { issuer: string; client_id: string };
export const listAccounts = async () => asJson<AccountEntry[]>(await api(`${BASE}/admin/users`));
/** A new account; the one-time link for them to set their password. */
export const inviteAccount = async (name: string, role: Role, email: string) =>
  asJson<{ name: string; invite: string }>(await post(`${BASE}/admin/users`, { name, role, email: email || null }));
export const changeAccount = async (name: string, change: { role?: Role; disabled?: boolean; email?: string }) =>
  asJson<AccountEntry>(await post(`${BASE}/admin/users/${encodeURIComponent(name)}`, change));
export const resetAccount = async (name: string) => asJson<{ name: string; invite: string }>(await post(`${BASE}/admin/users/${encodeURIComponent(name)}/reset`));
export async function removeAccount(name: string): Promise<void> {
  await asJson<void>(await api(`${BASE}/admin/users/${encodeURIComponent(name)}`, { method: "DELETE" }));
}
export const listTokens = async () => asJson<TokenEntry[]>(await api(`${BASE}/admin/tokens`));
export const createToken = async (label: string, role: Role) => asJson<{ token: string; entry: TokenEntry }>(await post(`${BASE}/admin/tokens`, { label, role }));
export async function revokeToken(id: string): Promise<void> {
  await asJson<void>(await api(`${BASE}/admin/tokens/${encodeURIComponent(id)}`, { method: "DELETE" }));
}
/** A folder an admin bound (holon-server's folders.rs): a name flows use, a real folder here;
 *  for the whole server (`deployment` null), or for that deployment only. What its reads keep in
 *  the run log (`bytes for 7d`: the bytes for a week, then a hash), and the most its files may
 *  hold in all, in bytes (`cap`, null: uncapped). */
export type FolderEntry = {
  name: string;
  path: string;
  access: "read" | "read-write";
  keep: "bytes" | "hash" | `bytes for ${string}`;
  cap: number | null;
  /** Other servers bind it too (a network share): a commit also locks a file in it. */
  shared?: boolean;
  /** "run": each run that touches it holds it for its whole length (shared to read, alone to write), not only across its commit. */
  lock?: "commit" | "run";
  deployment: string | null;
};
/** A folder to bind: its cap as bytes or a size (`10M`). */
export type FolderBinding = Omit<FolderEntry, "cap"> & { cap: number | string | null };
export const listFolders = async () => asJson<FolderEntry[]>(await api(`${BASE}/admin/folders`));
export const bindFolder = async (f: FolderBinding) => asJson<FolderEntry>(await post(`${BASE}/admin/folders`, f));
/** A folder on the server's machine, as its admin page's "browse…" lists it (pick.rs). */
export type FolderPlace = { name: string; path: string };
/** `GET /admin/folders/browse`: a folder's folders (none named: the home folder's), its parent, the
 *  machine's roots, and whether this editor may open the OS's own folder dialog there. */
export type FolderListing = { path: string; parent: string | null; folders: FolderPlace[]; more: boolean; roots: FolderPlace[]; native: boolean; native_refused?: string };
export const browseFolders = async (path?: string) =>
  asJson<FolderListing>(await api(`${BASE}/admin/folders/browse${path ? `?path=${encodeURIComponent(path)}` : ""}`));
/** `POST /admin/folders/pick`: the OS's folder dialog, on the server's machine; the folder chosen,
 *  or null if it was cancelled. */
export const pickFolder = async (start: string) => asJson<{ path: string | null }>(await post(`${BASE}/admin/folders/pick`, { start }));
export async function unbindFolder(name: string, deployment: string | null): Promise<void> {
  const only = deployment ? `?deployment=${encodeURIComponent(deployment)}` : "";
  await asJson<void>(await api(`${BASE}/admin/folders/${encodeURIComponent(name)}${only}`, { method: "DELETE" }));
}
/** A database an admin bound (holon-server's databases.rs): a name flows use (`database shop`),
 *  a SQLite file here; for the whole server (`deployment` null), or for that deployment only. */
export type DatabaseEntry = {
  name: string;
  engine: "sqlite" | "postgres" | "mysql" | "mongodb";
  /** A SQLite database's file. */
  path?: string;
  /** A Postgres, MySQL or MongoDB database's: where its URL is fetched from (env:…, vault:…), and where it points, without credentials. */
  connection?: string;
  address?: string;
  access: "read" | "read-write";
  /** "run": each run that touches it holds it for its whole length (shared to read, alone to write), not only across its commit. */
  lock?: "commit" | "run";
  deployment: string | null;
};
/** A database to bind: a SQLite file's path or a connection's source, with the admin's schema to run first (which creates a SQLite file if there is none). */
export type DatabaseBinding = Pick<DatabaseEntry, "name" | "access" | "deployment" | "lock"> & { path?: string; connection?: string; schema: string | null };
export const listDatabases = async () => asJson<DatabaseEntry[]>(await api(`${BASE}/admin/databases`));
export const bindDatabase = async (d: DatabaseBinding) => asJson<DatabaseEntry>(await post(`${BASE}/admin/databases`, d));
export async function unbindDatabase(name: string, deployment: string | null): Promise<void> {
  const only = deployment ? `?deployment=${encodeURIComponent(deployment)}` : "";
  await asJson<void>(await api(`${BASE}/admin/databases/${encodeURIComponent(name)}${only}`, { method: "DELETE" }));
}
/** `GET /admin/databases/<name>/tables`: a bound database's tables (a MongoDB's collections), read now, Holon's own left out. */
export async function listTables(name: string, deployment: string | null): Promise<string[]> {
  const only = deployment ? `?deployment=${encodeURIComponent(deployment)}` : "";
  return (await asJson<{ tables: string[] }>(await api(`${BASE}/admin/databases/${encodeURIComponent(name)}/tables${only}`))).tables;
}
/** A mail resource bound (mails.rs): where its SMTP server's URL is read from (env:…, vault:…) and
 *  where it points, without the login; its sender; and every address it may send to (empty when
 *  it only receives); the mailbox it receives in, if it does; and where its token comes from, for
 *  a server whose URL names a user and no password (OAuth: Microsoft 365, Google Workspace). */
export type MailEntry = { name: string; deployment: string | null; smtp: string; address: string; from: string; to: string[]; mailbox: Mailbox | null; oauth?: OAuthSource | null };
/** Where a mail's token comes from: where the token request is read from, and where it is sent
 *  (without the client's login or the query). */
export type OAuthSource = { source: string; address: string };
/** A mailbox a mail receives in: where its IMAP server's URL is read from, where it points, the
 *  folder read, where each message is moved after its run, whose verdict counts, and how often
 *  (seconds) it is looked at. */
export type Mailbox = { imap: string; address: string; folder: string; done: string; verdict_from?: string; every: number };
/** A mail to bind: `to` as addresses, `*@domain` or `*`, with commas; `imap` (with its folder,
 *  done and verdict_from, empty for the defaults) to receive; `oauth`, where its token request is
 *  read from. */
export type MailBinding = Pick<MailEntry, "name" | "deployment" | "smtp" | "from"> & { to: string; imap?: string; folder?: string; done?: string; verdict_from?: string; oauth?: string };
export const listMails = async () => asJson<MailEntry[]>(await api(`${BASE}/admin/mails`));
export const bindMail = async (m: MailBinding) => asJson<MailEntry>(await post(`${BASE}/admin/mails`, m));
export async function unbindMail(name: string, deployment: string | null): Promise<void> {
  const only = deployment ? `?deployment=${encodeURIComponent(deployment)}` : "";
  await asJson<void>(await api(`${BASE}/admin/mails/${encodeURIComponent(name)}${only}`, { method: "DELETE" }));
}
/** What a feed's connection does with what comes while its buffer is full: waits, drops it, or
 *  replaces the waiting event of its key. */
export type OnFull = "wait" | "skip" | "latest";
/** A feed bound (feeds.rs): where its URL is read from (env:…, vault:…) and where it points,
 *  without its login or query; the headers each request sends (each name, and where its value is
 *  read from); how many events may wait for their run, and what comes when they are that many;
 *  how long (seconds) it may say nothing. A WebSocket's: what it is sent when it opens and to
 *  keep it, the fields that are a message's id, the messages it runs, and where a feed that
 *  replays takes the id it is opened again from (`resume`). A Postgres channel's: the database
 *  it is on and the channel (`url` empty, `address` the database's and the channel), its id and
 *  the messages it runs as a WebSocket's. Any feed's: the field that numbers a message
 *  (`sequence`), each `key` on its own. */
export type FeedEntry = {
  name: string;
  deployment: string | null;
  url: string;
  address: string;
  headers: Record<string, string>;
  buffer: number;
  idle: number;
  on_full: OnFull;
  key?: string;
  sequence?: string;
  subscribe?: string[];
  heartbeat?: string;
  every?: number;
  id?: string[];
  only?: Record<string, string[]>;
  resume?: { query?: string; header?: string; message?: string[] };
  database?: string;
  channel?: string;
};
/** A feed to bind: what is left out takes its default. `id` is FIELD[,FIELD], `only` each
 *  FIELD=VALUE, `resume` query:NAME, header:NAME or each message:…, `idle` and `every` 90s, 2m.
 *  A Postgres channel's names its `database` and `channel` in place of `url` and `headers`. */
export type FeedBinding = {
  name: string;
  deployment: string | null;
  url?: string;
  headers?: Record<string, string>;
  database?: string;
  channel?: string;
  buffer?: number;
  idle?: string;
  on_full?: OnFull;
  key?: string;
  sequence?: string;
  subscribe?: string[];
  heartbeat?: string;
  every?: string;
  id?: string;
  only?: string[];
  resume?: string[];
};
export const listFeeds = async () => asJson<FeedEntry[]>(await api(`${BASE}/admin/feeds`));
export const bindFeed = async (f: FeedBinding) => asJson<FeedEntry>(await post(`${BASE}/admin/feeds`, f));
export async function unbindFeed(name: string, deployment: string | null): Promise<void> {
  const only = deployment ? `?deployment=${encodeURIComponent(deployment)}` : "";
  await asJson<void>(await api(`${BASE}/admin/feeds/${encodeURIComponent(name)}${only}`, { method: "DELETE" }));
}
/** What a deployment's connection to one feed its handlers listen to is doing (`GET
 *  /deployments/<slug>/feeds`): since when (ms), why it isn't connected, the id of the last event
 *  run on, how many wait, and since it opened how many its binding passed over (`--only`), its
 *  full buffer dropped (`skip`), newer ones of their key replaced (`latest`), and numbers the feed
 *  skipped (`--sequence`). */
export type FeedState = {
  feed: string;
  label: string;
  address?: string;
  status: "connecting" | "connected" | "reconnecting" | "closed";
  since?: number;
  error?: string;
  position?: string;
  waiting: number;
  passed_over?: number;
  missed?: number;
  replaced?: number;
  gaps?: number;
};
export async function feedStates(slug: string): Promise<FeedState[]> {
  return (await asJson<{ feeds: FeedState[] }>(await api(`${BASE}/deployments/${encodeURIComponent(slug)}/feeds`))).feeds;
}
/** A queue bound (queues.rs): where its URL is read from (env:…, vault:…) and the queue it points
 *  to, without its key; whether it is FIFO, the largest message it takes (bytes), how long a
 *  message received is kept out of sight at first (seconds), and whether a failed run's message is
 *  left in the queue for its redrive rather than deleted. */
export type QueueEntry = {
  name: string;
  deployment: string | null;
  url: string;
  address: string;
  fifo: boolean;
  largest: number;
  visibility: number;
  keep_failed: boolean;
};
/** A queue to bind: what is left out takes its default. `visibility` 5m, 90s. */
export type QueueBinding = { name: string; deployment: string | null; url: string; visibility?: string; keep_failed?: boolean };
export const listQueues = async () => asJson<QueueEntry[]>(await api(`${BASE}/admin/queues`));
export const bindQueue = async (q: QueueBinding) => asJson<QueueEntry>(await post(`${BASE}/admin/queues`, q));
export async function unbindQueue(name: string, deployment: string | null): Promise<void> {
  const only = deployment ? `?deployment=${encodeURIComponent(deployment)}` : "";
  await asJson<void>(await api(`${BASE}/admin/queues/${encodeURIComponent(name)}${only}`, { method: "DELETE" }));
}
/** What a deployment's listener to one queue its handlers listen to is doing (`GET
 *  /deployments/<slug>/queues`): since when (ms), why it can't receive, how many received wait
 *  for their run, and since it started how many ran, were passed over (run already) and were
 *  left for the queue's redrive (`keep_failed`). */
export type QueueListening = {
  queue: string;
  label: string;
  address?: string;
  status: "receiving" | "unreachable" | "stopped";
  since?: number;
  error?: string;
  waiting: number;
  run: number;
  passed_over?: number;
  kept?: number;
};
/** What a deployment's messages to one queue came to since the server started: how many it
 *  sent, gave up on, and wait to be sent. */
export type QueueSending = { queue: string; address?: string; sent: number; gave_up?: number; waiting: number };
export async function queueStates(slug: string): Promise<{ listening: QueueListening[]; sending: QueueSending[] }> {
  return asJson(await api(`${BASE}/deployments/${encodeURIComponent(slug)}/queues`));
}
/** Values and bytes, as a sweep counts them. */
export type Counted = { values: number; bytes: number };
/** `POST /admin/store/sweep` (sweep.rs): the values nothing kept names, removed (or, as a dry
 *  run, only counted); what it keeps, and what it spares because it was put in the last hour. */
export async function sweepStore(dryRun: boolean): Promise<{ dry_run: boolean; swept: Counted; kept: Counted; spared: Counted }> {
  return asJson(await post(`${BASE}/admin/store/sweep`, { dry_run: dryRun }));
}
/** `POST /admin/folders/sweep` (folders.rs): the file texts no live run, kept run or folder names. */
export async function sweepFolders(): Promise<{ texts: number; chunks: number; bytes: number; kept: { texts: number; chunks: number; bytes: number } }> {
  return asJson(await post(`${BASE}/admin/folders/sweep`, {}));
}
/** What one of the server's own sweeps did (storage.rs `Report`). */
export type SweepReport = { at_ms: number; took_ms: number; removed: number; bytes: number; skipped: boolean; error: string | null };
/** `GET /admin/host`'s sweeps: how often (`"10m"`, `"off"`), when the next is due, what the last of each did. */
export type HostSweeps = { every: string; next_at_ms: number | null; sweeping: boolean; store: SweepReport | null; folders: SweepReport | null };
/** Files, their bytes, and the disk they take in whole blocks (storage.rs `Count`). */
export type Counted3 = { files: number; bytes: number; on_disk: number };
/** A cache's use against its size. */
export type CacheUse = { entries: number; bytes: number; budget: number };
/** One disk the server keeps things on, with what it keeps there. */
export type HostDisk = { path: string; holds: string[]; size: number; free: number; used: number };
/** `GET /admin/host` (storage.rs): memory, disks, what the host keeps on them (the store and the
 *  folders' texts as of their last count), and how it sweeps. */
export type HostView = {
  memory: { resident: number | null; peak: number | null; flow_cache: CacheUse; value_cache: CacheUse };
  disks: HostDisk[];
  kept: {
    at_ms: number | null;
    store: { values: number; bytes: number; on_disk: number; put_since: number } | null;
    logs: Counted3 | null;
    file_texts: { texts: number; chunks: number; bytes: number } | null;
    leftovers: { removed: Counted3; waiting: Counted3 };
  };
  deployments: { each: Holds[]; shared_in_store: { values: number; bytes: number } | null };
  sweeps: HostSweeps;
  /** `[storage] warn_at`: `"85%"` full, or `"5G"` free. */
  warn_at: string;
  /** `[storage] full_at` and `when_full`: past it, what the server does. */
  full_at: string;
  when_full: "warn" | "pause" | "roll";
  /** What `when_full` has done and not yet undone. */
  acting: { paused: string[]; rolled: string[] };
  /** `[storage] backup`: where the runs a log lets go of go first, and what it holds; null: none. */
  backup: { dir: string; exports: number; bytes: number; waiting: number; error: string | null } | null;
  warnings: HostWarning[];
};
/** Something about the host an admin should know (storage.rs `Warning`), and what to do about it. */
export type HostWarning = { key: string; says: string; todo: string };
/** `GET /admin/warnings`: what the host warns of now. */
export const getWarnings = async () => (await asJson<{ warnings: HostWarning[] }>(await api(`${BASE}/admin/warnings`))).warnings;
export const getHost = async () => asJson<HostView>(await api(`${BASE}/admin/host`));
/** What one deployment holds, in memory and on disk (storage.rs `holds`): its log (in memory,
 *  about 3 times the lines its kept runs were written as; and its file), its live state, its flow
 *  in the cache, and its states and file texts as the store's last sweep found them. */
export type Holds = {
  slug: string;
  memory: number;
  disk: number;
  /** The runs it keeps, the newest `entries` of them in memory (the rest are read from its file). */
  log: { kept: number; entries: number; entry_bytes: number; memory: number; lines: number; file: Counted3 };
  state_memory: number;
  flow_cache: { own: number; shared: number } | null;
  store: { values: number; bytes: number } | null;
  file_texts: { texts: number; bytes: number } | null;
  shares_at_ms: number | null;
};
/** `GET /deployments/<slug>/holds`. */
export const getHolds = async (slug: string) => asJson<Holds>(await api(`${BASE}/deployments/${encodeURIComponent(slug)}/holds`));
/** `POST /admin/storage`: the server's `[storage]` settings, written into its `holon.toml`: how
 *  often it sweeps (`"10m"`, `"2h"`, `"off"`), when it warns its disk is full (`"85%"`, `"5G"`). */
export async function setStorage(set: { sweep_every?: string; warn_at?: string; full_at?: string; when_full?: string; backup?: string }): Promise<{ sweeps: HostSweeps; warnings: HostWarning[] }> {
  return asJson(await post(`${BASE}/admin/storage`, set));
}
export const setSweepEvery = (every: string) => setStorage({ sweep_every: every });
/** One deployment's grant as `GET /admin/grants` lists it (grants.rs): what it holds, what was taken back since. */
export type HeldGrant = { slug: string; status: DeploymentEntry["status"]; attention: string | null; items: string[]; revoked: string[] };
export const listGrants = async () => asJson<HeldGrant[]>(await api(`${BASE}/admin/grants`));
export const getOidcSettings = async () => (await asJson<{ settings: OidcSettings | null }>(await api(`${BASE}/admin/oidc`))).settings;
export async function setOidcSettings(settings: OidcSettings | null): Promise<void> {
  await asJson<void>(await post(`${BASE}/admin/oidc`, { settings }));
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
    throw new ServerError(res.status, message);
  }
  // 204: nothing to read.
  if (res.status === 204) return undefined as T;
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
};

/** `POST /deploy` — register `slug -> unit`. `acknowledge` must name every capability the
 *  unit's closure actually derives (via `Evaluator.capability_names`, computed by
 *  the caller from `Evaluator.capabilities()`) — the server re-derives the real set independently
 *  and checks it, never trusts this list as the truth. */
export async function deployUnit(
  slug: string,
  unitHash: string,
  acknowledge: string[],
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
      ...(document !== undefined ? { document } : {}),
      ...(requirements !== undefined ? { requirements } : {}),
      // What it places, beside the document (holonUsage.ts): who uses a holon, from listings.
      ...(document !== undefined ? { uses: usesIn(document) } : {}),
      // The web pages from other sites it answers (⚙), when it says any.
      ...(document?.server ? { browsers: browsersOf(document.server) } : {}),
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
  /** The actions agents may run (`action reset`'s `reset`): each is `AgentRuns("reset")`, a
   *  grant a person approves and can take back. A person runs any of them either way. */
  agents?: string[];
};

/** `POST /deploy/preview` — what deploying would ask for, without deploying. */
export type DeployPreview = {
  manifest: string;
  items: string[];
  /** What `/deploy` will want acknowledged. */
  needs_approval: string[];
  /** Granted already: by host policy, or by the deployment's current grant. */
  already_granted: string[];
  /** Where each secret it reads is sent (secret → hosts), said beside the secret's item. */
  sent?: Record<string, string[]>;
  /** Why the host would refuse it, if it would. */
  refused: string | null;
  /** Secrets it reads that aren't set yet: those nodes stay empty until they are. */
  missing_secrets: string[];
  /** What the slug's grant covers now (null: not deployed yet): beside `items`, what changes. */
  granted_now?: string[] | null;
  /** Secrets an agent's deploy leaves out, for a person to grant back. */
  withheld?: string[];
  /** Each thing it touches (a folder, a secret, a host…): new, gone or as now, and whether this
   *  server is ready for it (touches.ts). */
  touches?: Touch[];
  /** The deploy settings that change. */
  settings?: SettingChange[];
};

/** A secret as the server lists it: never its value. `source`: `local` (sealed by the server),
 *  `env:HOLON_SECRET_…` or `vault:<mount>/<path>#<field>`. `problem`: why it has no value now. */
/** One secret set, never its value. `server`: the server's, read by a deployment that has none of
 *  its own; `to`: where the server's may go, when an admin said (hosts, `db:` databases). */
export type SecretInfo = { name: string; version: number; source: string; set_at: number; problem?: string; server?: boolean; to?: string[] };

/** `GET /admin/secrets` (admins only) — the server's own secrets, each with where it may go. */
export async function listServerSecrets(): Promise<SecretInfo[]> {
  return (await asJson<{ secrets: SecretInfo[] }>(await api(`${BASE}/admin/secrets`))).secrets;
}

/** Bind the server's secret `name`, as a new version (a value, or where it is kept), and where it
 *  may go (`to`: hosts and `db:` databases; absent: wherever its flows' approvals show). */
export async function bindServerSecret(name: string, set: ({ value: string } | { from: string }) & { to?: string[] }): Promise<number> {
  const res = await api(`${BASE}/admin/secrets/${encodeURIComponent(name)}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(set) });
  return (await asJson<{ version: number }>(res)).version;
}

/** Change only where the server's secret `name` may go (null: wherever its flows' approvals show). */
export async function capServerSecret(name: string, to: string[] | null): Promise<void> {
  await asJson(await api(`${BASE}/admin/secrets/${encodeURIComponent(name)}/cap`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ to }) }));
}

/** Remove the server's secret `name`, every version. */
export async function unbindServerSecret(name: string): Promise<void> {
  const res = await api(`${BASE}/admin/secrets/${encodeURIComponent(name)}`, { method: "DELETE" });
  if (!res.ok) throw new Error(await res.text());
}

/** `GET /deployments/<slug>/secrets` — the secrets set, never their values. */
export async function listSecrets(slug: string): Promise<SecretInfo[]> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/secrets`);
  return (await asJson<{ secrets: SecretInfo[] }>(res)).secrets;
}

/** `GET /admin/secrets/env` (admins only) — the `HOLON_SECRET_…` names an env secret can be read
 *  from: the server's environment and its dotenv file, read again first. Names, never values. */
export const getEnvSecretNames = async () => asJson<{ names: string[]; dotenv: string | null }>(await api(`${BASE}/admin/secrets/env`));

/** A secret's address: a deployment's, or (`slug` null) the server's own, an admin's. */
const secretUrl = (slug: string | null, name: string) =>
  slug === null ? `${BASE}/admin/secrets/${encodeURIComponent(name)}` : `${BASE}/deployments/${encodeURIComponent(slug)}/secrets/${encodeURIComponent(name)}`;

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

/** `GET /auth/config`: where to sign in, when the server has an identity provider; else null. */
export async function getAuthConfig(): Promise<AuthConfig | null> {
  const res = await fetch(`${BASE}/auth/config`).catch(() => null);
  return res && res.ok ? ((await res.json()) as AuthConfig) : null;
}

/** One thing done on the server: a save, deploy, run, pause, stop, grant or delete, and by whom. */
export type ActivityEntry = { at: number; action: string; target: string; by?: string };

/** `GET /activity`: what was done, newest first. */
export async function getActivity(limit = 30): Promise<ActivityEntry[]> {
  return (await asJson<{ entries: ActivityEntry[] }>(await api(`${BASE}/activity?limit=${limit}`))).entries;
}

/** What a check of every run log found (`POST /summary/check`): the lines it read, each run whose
 *  line doesn't name the hash of the one before, and each log's head as read. */
export type ChainCheck = { at_ms: number; runs: number; breaks: { slug: string; seq: number }[]; heads: [string, string][] };

/** The host at a glance (`GET /summary`, holon-server's summary.rs): `hours` hours ending at
 *  `at_ms`, each deployment's runs and failures in each (the oldest first), the totals, the
 *  newest runs across every flow, and the run logs' chain. */
export type Summary = {
  at_ms: number;
  hours: number;
  totals: { runs: number; failed: number; running: number; paused: number; stopped: number };
  deployments: {
    slug: string;
    status: "running" | "paused" | "stopped";
    runs: number[];
    failed: number[];
    /** Its unrecorded handlers' runs, all told: never logged, so in no hour. */
    unrecorded: { runs: number; failed: number };
    on_record: number;
    head: string;
    breaks: number[];
  }[];
  latest: { slug: string; seq: number; at_ms: number; duration_ms: number; kind: string; route: string | null; status: number | null; failed: boolean; waited?: boolean }[];
  chain: {
    on_record: number;
    breaks: number;
    /** Where the heads go, when it last took a batch (since the server started), and how many wait. */
    witness: { address: string; taken_ms: number | null; waiting: number } | null;
    checked: ChainCheck | null;
  };
};
export const getSummary = async (hours = 24) => asJson<Summary>(await api(`${BASE}/summary?hours=${hours}`));
export const checkChains = async () => asJson<ChainCheck>(await post(`${BASE}/summary/check`));

/** A version someone asks a person to deploy (proposals.rs): an agent's deploy that needs a
 *  person's approval, with its why and note, and what became of it. */
export type Proposal = {
  id: number;
  slug: string;
  unit: string;
  why: string;
  note?: string;
  by?: string;
  agent: boolean;
  at_ms: number;
  /** What it asked to be approved when it was made. */
  needs: string[];
  /** The stamp of the save of the flow it made. */
  saved_at?: number;
  state: "waiting" | "approved" | "declined" | "replaced";
  decided_by?: string;
  decided_at_ms?: number;
  reason?: string;
  /** While it waits: the version it would deploy as. */
  version?: number;
  /** Its canvas, with its text (`GET /proposals/<id>` only). */
  document?: FlowDocument;
};

/** `GET /proposals`: the waiting ones, oldest first (the queue), then the decided, newest first. */
export const listProposals = async () => (await asJson<{ proposals: Proposal[] }>(await api(`${BASE}/proposals`))).proposals;
/** `GET /proposals/<id>`: one, with its document. */
export const getProposal = async (id: number) => asJson<Proposal>(await api(`${BASE}/proposals/${id}`));
/** Decline a waiting proposal, with a reason its author reads. */
export const declineProposal = async (id: number, reason: string) => asJson<Proposal>(await post(`${BASE}/proposals/${id}/decline`, { reason }));
/** Whether agents may propose a deployment's versions. */
export const setAgentsPropose = async (slug: string, on: boolean) =>
  asJson<DeploymentEntry>(await post(`${BASE}/deployments/${encodeURIComponent(slug)}/settings`, { agents_propose: on }));

/** A secret's history: a deployment's, or (`slug` null) the server's own. */
export async function getSecretHistory(slug: string | null, name: string): Promise<SecretHistory> {
  return asJson<SecretHistory>(await api(secretUrl(slug, name)));
}

/** Make `version` current again, as a new version; answers it. */
/** What rolling a secret back would do, changing nothing: the version made current again, the
 *  one current now, the new version it would be, and its source. Never a value. */
export type SecretRollbackPreview = {
  to: number;
  current: number;
  version: number;
  source: string;
  /** Each value's fingerprint, as the log marks it (`#a1b2c3d4`), never the value; the current
   *  one's is absent when it can't be read now. */
  to_print?: string;
  current_print?: string | null;
};

/** `POST …/secrets/<name>/rollback` with `dry_run`: refused as the roll back would be. */
export async function previewSecretRollback(slug: string | null, name: string, version: number): Promise<SecretRollbackPreview> {
  const res = await api(`${secretUrl(slug, name)}/rollback`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ version, dry_run: true }),
  });
  return asJson<SecretRollbackPreview>(res);
}

export async function rollbackSecret(slug: string | null, name: string, version: number): Promise<number> {
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
  /** `[agents] propose`: whether an agent's version that needs a person waits for one as a
   *  proposal (off: it is refused, and nothing is saved). */
  agents_propose?: boolean;
  /** `[storage]`: how often the server sweeps (`"10m"`, `"off"`). */
  storage?: { sweep_every: string; warn_at?: string };
};

export async function getPermissions(): Promise<HostPolicy> {
  return asJson<HostPolicy>(await api(`${BASE}/permissions`));
}

/** `POST /permissions/reload`: the server reads its policy file again and holds every deployment
 *  to it; the policy, and why each deployment can't run, if it can't. */
export async function reloadPermissions(): Promise<HostPolicy & { attention: Record<string, string | null> }> {
  return asJson(await post(`${BASE}/permissions/reload`));
}

/** One entry from `GET /deployments` (or `DELETE /deployments/<slug>`'s response) — mirrors
 *  `DeploymentEntry` exactly, including the state/lifetime fields that came with real
 *  per-deployment state. */
export type DeploymentEntry = {
  slug: string;
  unit: string;
  triggers: string[];
  has_state: boolean;
  trigger_count: number;
  error_count: number;
  /** Its unrecorded handlers' runs, counted, by label: absent while it has none. */
  tallies?: Record<string, Tally>;
  deployed_at_epoch_seconds: number;
  /** Who deployed it (a named user, or `admin`), when the server knows. */
  deployed_by?: string;
  /** The saved flows its version places. */
  uses?: Placed[];
  /** The port a server deployment answers on; null for any other deployment. */
  port: number | null;
  status: DeploymentStatus;
  /** The current session's number (the last one's when stopped). */
  session: number;
  sessions: number;
  /** Why it can't run until deployed again (its grant no longer covers what it needs). */
  attention?: string;
  /** Why a server deployment isn't answering on its port (it couldn't listen there, say another
   *  program holds it), until a deploy or run listens there again. */
  listen_error?: string;
  /** How many of its runs are waiting for an approval or an event. */
  waiting?: number;
  /** Whether agents may propose its versions (off: an agent's deploy of it is refused). */
  agents_propose?: boolean;
};

/** A run waiting for an approval or an event (`GET /deployments/<slug>/runs`; never its tokens). */
export type WaitingRun = {
  run_id: string;
  kind: string;
  route: string | null;
  parked_at: number;
  /** Each point it waits at; an approval's with how many people it needs and who approved so far. */
  waiting: { name: string; expires_at: number; people?: number; approved?: { by: string; at: number; reason?: string }[] }[];
  delivered: string[];
};

/** What a decision did (`POST …/runs/<run_id>/decide`): the approval still waits for more people,
 *  or the run was given it (`run_error` when its part then failed). */
export type Decided = { name: string; people: number; approved_by: string[]; outcome: "waiting" | "approved" | "declined"; run_error?: string };

/** Approve, or decline with a reason, the approval `name` a run waits at: a person's to do. */
export const decideRun = async (slug: string, runId: string, name: string, approve: boolean, reason: string) =>
  asJson<Decided>(await post(`${BASE}/deployments/${encodeURIComponent(slug)}/runs/${encodeURIComponent(runId)}/decide`, { name, approve, reason: reason.trim() || undefined }));

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
  /** The hash of the log's line before it (hex): the chain a witness checks. Absent for the
   *  first entry. */
  prev?: string;
  /** The unit it ran (hex): the deployment's version then. Absent in entries from before. */
  unit?: string;
  session: number;
  at_ms: number;
  duration_ms: number;
  kind: "request" | "trigger" | "tick" | "resume" | "file" | "mail" | "feed" | "queue" | "websocket";
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
  /** What it read: collection items and files. A file the log keeps by hash has no answer, and
   *  says so in `file` (its BLAKE3 hash and size, and where its text is kept, if it is). */
  reads?: { path: number[]; request: WValue; answer: WValue | null; file?: { hash: string; size: number; blob?: string } }[];
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
  /** The secrets the run was given: the version and fingerprint their marks name (`v3 #a1b2c3d4`). */
  secrets?: Record<string, string>;
  /** The state it left, by hash in the content store: what a restore to this run puts back.
   *  Absent when it didn't commit, and in runs from before rc.30. */
  state_after?: string;
};

/** The requests one round made: each node (by its path in the run's root unit), its request,
 *  its response, and when, in microseconds from the run's start. */
export type LoggedRound = {
  exchanges: { path: number[]; request: WValue; response: WValue; start_us: number; end_us: number }[];
};

/** What the log doesn't hold of an unrecorded handler's runs: how many ran, how many failed, how
 *  long they took in all, when the last one started, and the last failure's time, status and error
 *  (never its request): none of its runs is logged, failed or not. */
export type Tally = { runs: number; failed: number; total_ms: number; last_at_ms: number; last_failure?: { at_ms: number; status: number; error?: string } };

/** `GET /deployments/<slug>/log` — entries past `after` (all without), oldest first. `next` is
 *  the seq the next entry will get; `head` the hash the log chains up to (what a witness holds),
 *  `breaks` the runs that don't follow the one before them (changed outside holon); `tallies` the
 *  counts of its unrecorded handlers' runs, by label (absent from a server before them). */
export async function getLog(slug: string, after?: number): Promise<{ entries: LogEntry[]; next: number; head: string; breaks: number[]; tallies?: Record<string, Tally> }> {
  const q = after === undefined ? "" : `?after=${after}`;
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/log${q}`);
  return asJson(res);
}

/** A line a flow wrote with `console.log` and the rest, kept beside its runs and not in them (logs.rs):
 *  its time, level (`debug`, `log`, `info`, `warn`, `error`), the version that wrote it, the run, and its
 *  text; `dropped` more than 0: it stands for that many dropped before it. */
export type LogLine = { at: number; level: string; unit: string; run: number; text: string; dropped?: number };

/** A deployment's lines, oldest first: at `level` or louder, one run's (`run`) or every line up to the end
 *  of one's (`until`), after the time `after` (ms), the newest `limit`. An agent is refused them. */
export async function getLogs(slug: string, q: { level?: string; run?: number; until?: number; after?: number; limit?: number } = {}): Promise<LogLine[]> {
  const asked = Object.entries(q).filter(([, v]) => v !== undefined).map(([k, v]) => `${k}=${encodeURIComponent(String(v))}`);
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/logs${asked.length > 0 ? `?${asked.join("&")}` : ""}`);
  return (await asJson<{ lines: LogLine[] }>(res)).lines;
}

/** One run a version was replayed on: what it did then against what the version does with it. */
export type RunReplayed = {
  seq: number;
  at_ms: number;
  kind: string;
  route?: string;
  outcome: "same" | "differs" | "cant_tell";
  /** How it differs, each a phrase; or why it can't be told, one. */
  why?: string[];
  /** A request's status then, and the version's. */
  status?: number;
  now_status?: number;
};

/** `GET /deployments/<slug>/replay`: a version replayed offline on the deployment's recent runs. */
export type Replayed = { unit: string; replayed: number; same: number; differ: number; cant_tell: number; runs: RunReplayed[] };

/** `GET /deployments/<slug>/replay?unit=<hash>` — the version `unit` (staged, or a proposal's)
 *  replayed on the deployment's last `runs` runs, offline: nothing is sent or changed. */
export async function replayVersion(slug: string, unit: string, runs = 20): Promise<Replayed> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/replay?unit=${encodeURIComponent(unit)}&runs=${runs}`);
  return asJson(res);
}

/** `GET /deployments/<slug>/export` — the deployment's runs as one file (its log, exactly as
 *  written, and every unit they ran), saved as `<slug>-runs.json` for `holon verify`. */
export async function downloadRuns(slug: string): Promise<void> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/export`);
  // A server from before exports answers the route with a bare 404 (a missing deployment's has a
  // reason): say which it is.
  if (res.status === 404 && !(await res.clone().text()).trim()) {
    throw new Error("this server can't export runs yet: it is older than this editor; update it (holon update) and start it again");
  }
  if (!res.ok) await asJson(res);
  const url = URL.createObjectURL(await res.blob());
  const a = document.createElement("a");
  a.href = url;
  a.download = `${slug}-runs.json`;
  a.click();
  URL.revokeObjectURL(url);
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

/** One version a deployment was deployed with (`GET /deployments/<slug>/versions`). */
export type VersionEntry = {
  number: number;
  unit: string;
  /** Unix seconds. */
  deployed_at: number;
  by?: string | null;
  /** The version whose code it restored, when it was a restore. */
  restored_from?: number | null;
  /** The one deployed now. */
  current: boolean;
  /** What its deploy asked is kept, so its code can be restored (rc.30 on). */
  restorable: boolean;
  /** Its canvas is kept. */
  canvas: boolean;
  /** The state it left is kept, so it can be put back. */
  state_kept: boolean;
  /** The log's seq at its deploy: its first run's (absent from before rc.30). */
  first_seq?: number | null;
};

/** `GET /deployments/<slug>/versions` — every version, newest first. */
export async function getVersions(slug: string): Promise<VersionEntry[]> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/versions`);
  return (await asJson<{ versions: VersionEntry[] }>(res)).versions;
}

/** One change to a cell (changes.rs): a run that changed it (`seq`), or something between two
 *  runs that moved it (`between`, with the events there: a restore, a deploy). Its value before
 *  and after; for a whole collection, the items it changed instead. */
export type CellChange = {
  seq?: number;
  between?: [number, number];
  events?: { kind: string; what: string; by?: string | null; at_ms: number }[];
  at_ms?: number | null;
  kind?: string;
  route?: string | null;
  method?: string | null;
  url?: string | null;
  status?: number | null;
  failed?: boolean;
  unit?: string;
  by?: string | null;
  before?: WValue | null;
  after?: WValue | null;
  items?: Record<string, { before: WValue | null; after: WValue | null }>;
  more_items?: boolean;
};

/** A cell's history: `GET /deployments/<slug>/changes`, newest first. */
export type CellHistory = {
  name: string;
  item: string | null;
  collection: boolean;
  /** Its value now (none for a whole collection). */
  now: WValue | null;
  changes: CellChange[];
  /** Older changes not answered: `before` the oldest one's seq reads them. */
  more: boolean;
};

/** The runs that changed a cell (`state.name`), a collection, or one item of it (`item`). */
export async function getChanges(slug: string, name: string, opts: { item?: string; before?: number; limit?: number } = {}): Promise<CellHistory> {
  const q = new URLSearchParams({ name });
  if (opts.item !== undefined) q.set("item", opts.item);
  if (opts.before !== undefined) q.set("before", String(opts.before));
  if (opts.limit !== undefined) q.set("limit", String(opts.limit));
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/changes?${q}`);
  // A server from before answers the route with a bare 404 (a missing deployment's has a reason).
  if (res.status === 404 && !(await res.clone().text()).trim()) {
    throw new Error("this server can't show a cell's history yet: it is older than this editor; update it (holon update) and start it again");
  }
  return asJson<CellHistory>(res);
}

/** Something done to a deployment (events.rs): a deploy, a restore, a grant, a secret, a binding,
 *  a pause or a run, by whom (none: the host did it). */
export type DeploymentEvent = {
  seq: number;
  /** The hash of the event before it in its chain. */
  prev?: string;
  at_ms: number;
  /** The run log's next seq when it happened: it comes before that run. */
  run_seq: number;
  kind: "deploy" | "restore" | "ask" | "grant" | "secret" | "binding" | "lifecycle" | string;
  what: string;
  by?: string;
  version?: number;
  session?: number;
  name?: string;
  /** The point whose state a restore put back: `version 3`, `run 41`; an ask's point. */
  from?: string;
  /** What an ask (an agent's restore, saved for a person to approve) would restore of its point. */
  restores?: ("code" | "state")[];
};

/** `GET /deployments/<slug>/events` — everything done to it, oldest first. A server from before
 *  keeps none: it answers the route with a bare 404, and this says so. */
export async function getEvents(slug: string): Promise<{ events: DeploymentEvent[]; head: string; breaks: number[] }> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/events`);
  if (res.status === 404 && !(await res.clone().text()).trim()) {
    throw new Error("this server keeps no history of what was done to a deployment yet: update it (holon update) and start it again");
  }
  return asJson(res);
}

/** A point to go back to: a version (the state it left, and its code) or a run (the state it left,
 *  and the code it ran on). */
export type RestorePoint = { version: number } | { run: number };

/** What putting a point's state back would change (a restore's dry run). */
export type StateChange = {
  from: string;
  changed: string[];
  added: string[];
  removed: string[];
  runs_since: number;
  /** Outside effects the runs since performed, by capability: a restore doesn't undo them. */
  effects_since: Record<string, number>;
  /** The handlers kept unrecorded that do something outside its state: their runs aren't counted
   *  in the runs since. */
  unrecorded?: string[];
  /** Each key it touches (the first 50), as it is now and as it would be; null: absent. A
   *  collection's are in `collections` instead. */
  values?: Record<string, { now: WValue | null; then: WValue | null }>;
  /** Each collection it touches, by name: the items that differ, by key; `more` when it shows or
   *  compared only some. */
  collections?: Record<string, { items: Record<string, { now: WValue | null; then: WValue | null }>; more: boolean }>;
  /** The files the runs since wrote, which a restore never puts back; `files_more` when there are
   *  more than it names. */
  files?: FileSince[];
  files_more?: boolean;
};

/** What a run's file write left: its kind, and the content's hash and size (none for a delete). */
export type FileWritten = { kind: "write" | "copy" | "delete"; hash: string | null; size: number | null; run: number };

/** A file the runs since a point wrote, as a restore's dry run names it: how many writes and by
 *  which runs (the last ten), what the last left, what the last write before the point left
 *  (`at_point`: none if no run wrote it before), and how it is on disk now (`disk`: none if it
 *  isn't there; `unreadable`: why it can't be checked). `outside`: the disk isn't what the last
 *  write left. Its text then and now, when the log has it and it is small. */
export type FileSince = {
  folder: string;
  name: string;
  writes: number;
  runs: number[];
  last: FileWritten;
  at_point: FileWritten | null;
  disk: { hash: string; size: number | null } | null;
  unreadable: string | null;
  outside: boolean;
  then_text: string | null;
  now_text: string | null;
};

/** A restore's dry run: the deploy's preview when it deploys code (`code`: the version), and the
 *  state change when it puts state back. */
export type RestorePreview = Partial<DeployPreview> & { code: number | null; state?: StateChange };

/** What a restore did: the version it deployed (as `version`, restoring `code`), and the session
 *  the state put back started. */
export type RestoreDone = { code?: number; version?: number; unit?: string; state?: { from: string; session: number } };

/** `POST /deployments/<slug>/restore` — go back to `point`: its code, its state, or both. With
 *  `dryRun`, it changes nothing and says what it would; else `acknowledge` is what the dry run
 *  asked to approve. */
export async function restoreDeployment(
  slug: string,
  point: RestorePoint,
  what: { code: boolean; state: boolean },
  opts: { dryRun: true } | { acknowledge: string[] },
): Promise<RestorePreview | RestoreDone> {
  const body = { ...point, ...what, ...("dryRun" in opts ? { dry_run: true } : { acknowledge: opts.acknowledge }) };
  return asJson(await post(`${BASE}/deployments/${encodeURIComponent(slug)}/restore`, body));
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
 *  ever acts on what's currently in the table, and its routes and actions start 404ing
 *  immediately afterward. */
export async function deleteDeployment(slug: string): Promise<DeploymentEntry> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}`, { method: "DELETE" });
  return asJson<DeploymentEntry>(res);
}

/** One `(capability, payload)` pair a trigger actually performed — mirrors `PerformedEntry`. */
export type PerformedEntry = { capability: string; payload: WValue };

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

/** `POST /deployments/<slug>/tick` — that clock of a deployed flow (its label, `every 2s`) ticks
 *  now, its next tick a period later; answers the tick's history entry. */
export async function tickNow(slug: string, clock: string): Promise<{ at_epoch_seconds: number; label?: string; performed: unknown[] }> {
  const res = await api(`${BASE}/deployments/${encodeURIComponent(slug)}/tick`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ clock }),
  });
  return asJson(res);
}

/** Where a deployment's action `name` is run: `POST <server>/trigger/<slug>/<name>`. */
export function actionUrl(slug: string, name: string): string {
  return `${BASE || window.location.origin}/trigger/${encodeURIComponent(slug)}/${encodeURIComponent(name)}`;
}

/** `POST /trigger/<slug>/<name>` — run the deployment's action `name` by hand, `sent` what it is
 *  sent (JSON text; empty is `{}`): its answer, and the seq of the run's log entry. Throws
 *  {@link ServerError}: 400 for what isn't JSON, 404 for no such action, 403 for a viewer. */
export async function runAction(slug: string, name: string, sent: string): Promise<{ answer: unknown; seq: number; waiting?: boolean }> {
  const res = await api(actionUrl(slug, name), { method: "POST", headers: { "Content-Type": "application/json" }, body: sent });
  return asJson(res);
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
  /** The module units its composites place (a js node's engine), by engine and params: rebuilt
   *  on open from the engine the bundle carries (docUnits.ts), checked by hash. */
  modules?: ModuleEntry[];
  /** What its composites are called, by hash (docUnits.ts): a unit keeps no name, so a holon
   *  inside another would otherwise show as its hash once reopened. Absent in older flows. */
  names?: Record<string, string>;
  /** The flow's deploy settings (DeployBar): the port for a flow with routes. Absent in older
   *  flows; `endpoints`, from flows saved before routes were derived, is ignored. */
  server?: { port: string; endpoints?: EndpointDraft[]; requirements?: Requirements; browsers?: string };
  /** For a deployed flow: which canvas node each step of a run is, by route (`"*"` for a flow
   *  deployed whole), so the log's timings land on the canvas (replay.ts's `nodeTimings`). */
  trace?: Record<string, string[]>;
  /** The flow's text as its author typed it (the text pane), comments and all, when it stands
   *  for this canvas; absent when it was never typed (the text is the canvas printed). */
  source?: string;
  /** For a deployed flow: the name the text binds each top-level node to, by node id, so a run
   *  explains as the text reads (holonText.ts `bindingsOf`; `holon mcp` deploys the same). */
  bindings?: Record<string, string>;
};

/** One entry in `GET /flows` — name + freshness only, never the (possibly large) document. */
/** `saved_by`: who saved it (a named user, or `admin`), when the server knows. `uses`: the saved
 *  flows it places (absent when saved before that was recorded). */
export type FlowListEntry = { name: string; saved_at_epoch_seconds: number; saved_by?: string; uses?: Placed[] };

/** A full saved flow as `GET /flows/<name>` returns it. */
export type FlowRecord = FlowListEntry & { document: FlowDocument };

/** `POST /flows` — save (upsert) the current canvas under `name`. The same name saves over
 *  the previous version: a flow is a mutable workspace file, and "save" means exactly what it
 *  means in any editor. Throws {@link ServerError} (400) for an invalid name. */
export async function saveFlow(name: string, document: FlowDocument): Promise<FlowListEntry> {
  const res = await api(`${BASE}/flows`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    // What it places, beside the document (holonUsage.ts): who uses a holon, from listings.
    body: JSON.stringify({ name, document, uses: usesIn(document) }),
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

/** `DELETE /flows/<name>` — remove a saved flow. Throws (404) if it was never there. */
export async function deleteFlow(name: string): Promise<void> {
  const res = await api(`${BASE}/flows/${encodeURIComponent(name)}`, { method: "DELETE" });
  await asJson<{ deleted: string }>(res);
}