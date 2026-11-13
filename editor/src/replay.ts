// Replaying a logged run on the canvas, node by node. A flow log entry (holon-server's
// recorder.rs) holds everything the run depended on: the request record, the state it read, and
// the sourced values (clock, random, the request, the new file). Evaluation is deterministic, so feeding those back
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

import { asksHostPrimitives, choiceNext, type WParam, type WPort, type WValue } from "./holon";
import { components } from "./derive";
import { stable } from "./textSync";

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
  /** The secrets the run was given: the version and fingerprint its marks name (`v3 #a1b2c3d4`). */
  secrets?: Record<string, string>;
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
      return v.value.includes("‹secret:") || v.value.includes("‹token:") || v.value.includes("‹file:");
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

/** Which of the deployed flow's traces places a run's steps, and how deep they sit: a request's
 *  route, or a clock's, file handler's, websocket block's or action's label (`every 2s`, `on file
 *  inbox`, `WebSocket /room/:name message`, `action reset`), one level down, inside its handler
 *  unit; else the whole flow's (`"*"`), at the top. */
function placedBy(entry: { kind: string; route: string | null }): { key: string; depth: 1 | 2 } {
  const handler = entry.kind === "request" || ((entry.kind === "tick" || entry.kind === "file" || entry.kind === "trigger" || entry.kind === "websocket") && !!entry.route);
  return handler ? { key: entry.route ?? "", depth: 2 } : { key: "*", depth: 1 };
}

/** A run's step paths, as the server records them, against canvas node ids. `trace` comes with
 *  the deployed flow: for each route or clock (or `"*"` for a whole-flow unit), which canvas node
 *  is each child of the unit as stored (canonical order). A request's or a clock's steps sit one
 *  level down, inside its handler unit (`[0, k]`); a whole-flow unit's at the top (`[k]`). */
function nodeAtPath(
  entry: { kind: string; route: string | null },
  trace: Record<string, string[]> | undefined,
): (path: number[]) => string | undefined {
  const { key, depth } = placedBy(entry);
  const map = trace?.[key];
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
  const { key, depth } = placedBy(entry);
  const map = trace?.[key];
  const k = map ? map.indexOf(nodeId) : -1;
  if (k < 0) return [];
  const prefix = depth === 2 ? [0, k] : [k];
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

/** What each `http_request` (or `http_upload`, `http_stream`, `flow_call`, `flow_start`) node was answered, from the run's record: the last answer it got,
 *  which is the one its final round used. Replay feeds these back instead of calling anything. */
export function recordedResponses(exchanges: NodeExchange[]): Map<string, WValue> {
  const out = new Map<string, WValue>();
  for (const x of exchanges) out.set(x.id, x.response);
  return out;
}

/** The lines of a span of a file, without their line endings, as the server reads them
 *  (ranges.rs `lines_of`). */
export function linesOf(text: string): string[] {
  return (text.match(/[^\n]*\n|[^\n]+$/g) ?? []).map((line) => line.replace(/\r?\n$/, ""));
}

/** What each collection or file read (`collection_get`, `collection_list`, `file_read`,
 *  `file_list`, `file_handle`, `file_lines`, `file_slice`) was answered, by canvas node, from the
 *  run's record: replay answers them from here, as the run found the collection or folder. A read
 *  that found nothing is left out, and replays as nothing; part of a file answers with the text
 *  the log keeps with its range (or the lines of it); a file or a range the log keeps by hash
 *  replays as a placeholder naming it, as a secret does, and what it feeds shows what the run
 *  recorded. */
export function recordedReads(
  entry: {
    kind: string;
    route: string | null;
    reads?: { path: number[]; request?: WValue; answer: WValue | null; file?: { hash: string; size: number; range?: { start: number; end: number; text?: string } } }[];
  },
  trace: Record<string, string[]> | undefined,
): Map<string, WValue> {
  const out = new Map<string, WValue>();
  const at = nodeAtPath(entry, trace);
  const field = (v: WValue | null | undefined, k: string) => (v?.type === "Record" ? (v.value.find(([x]) => x === k)?.[1] ?? null) : null);
  const text = (v: WValue | null) => (v?.type === "Text" ? v.value : "?");
  for (const r of entry.reads ?? []) {
    const id = at(r.path);
    if (!id) continue;
    const folder = text(field(r.request, "folder"));
    const range = r.file?.range;
    if (range) {
      const lines = field(r.request, "lines");
      const name = text(field(lines ?? field(r.request, "slice"), "name"));
      if (range.text === undefined) out.set(id, { type: "Text", value: `‹file:${folder}/${name}, bytes ${range.start}..${range.end}, kept by hash›` });
      else if (lines) out.set(id, { type: "List", value: linesOf(range.text).map((value): WValue => ({ type: "Text", value })) });
      else out.set(id, { type: "Text", value: range.text });
    } else if (r.answer !== null) out.set(id, r.answer);
    else if (r.file) out.set(id, { type: "Text", value: `‹file:${folder}/${text(field(r.request, "read"))}, ${r.file.size} bytes, kept by hash›` });
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

/** How a clock's period is written in its label: `2s`, `5m`, `1h`, `1d`, the largest unit it is a
 *  whole number of (core's `period_text`). */
export function periodText(seconds: number): string {
  for (const [per, unit] of [[86_400, "d"], [3_600, "h"], [60, "m"]] as const) {
    if (seconds % per === 0) return `${seconds / per}${unit}`;
  }
  return `${seconds}s`;
}

/** An `on_file` node's label, as the log names its handler's runs: `on file inbox`, `on file
 *  inbox deleted`, `on file inbox matching "*.csv"` (core's `FileTrigger::label`). */
export function fileLabel(params: ReplayNode["data"]["params"]): string {
  const matching = frozen(params, "matching") ?? "";
  return `on file ${frozen(params, "folder") ?? ""}${frozen(params, "on") === "deleted" ? " deleted" : ""}${matching ? ` matching ${JSON.stringify(matching)}` : ""}`;
}

/** An `on_mail` node's label, as the log names its handler's runs: `on mail remit` (core's
 *  `mail_label`). */
export function mailLabel(params: ReplayNode["data"]["params"]): string {
  return `on mail ${frozen(params, "mail") ?? ""}`;
}

/** An `on_feed` node's label, as the log names its handler's runs: `on feed prices` (core's
 *  `feed_label`). */
export function feedLabel(params: ReplayNode["data"]["params"]): string {
  return `on feed ${frozen(params, "feed") ?? ""}`;
}

/** A `websocket` node's label, as the log names its block's runs: `WebSocket /room/:name message`
 *  (core's `socket_label`). */
export function socketLabel(params: ReplayNode["data"]["params"]): string {
  return `WebSocket ${frozen(params, "path") ?? ""} ${frozen(params, "on") ?? ""}`;
}

/** An `on_queue` node's label, as the log names its handler's runs: `on queue payout_events`
 *  (core's `queue_label`). */
export function queueLabel(params: ReplayNode["data"]["params"]): string {
  return `on queue ${frozen(params, "queue") ?? ""}`;
}

/**
 * Which nodes a run touched: for a request, the group holding the request node of its route
 * (`"GET /todos/:id"`); for a clock's tick, the group holding its `every` node (`"every 2s"`);
 * for a file handler's run, the group holding its `on_file` node (`"on file inbox"`), a mail
 * handler's its `on_mail` node (`"on mail remit"`), a feed handler's its `on_feed` node (`"on feed
 * prices"`), a queue handler's its `on_queue` node (`"on queue payout_events"`), a websocket
 * block's its `websocket` node (`"WebSocket /room/:name message"`); for an action's, the group
 * holding its
 * `action` node (`"action reset"`); for anything else (a flow's tick, a trigger, a flow of
 * `on file` lines), the whole canvas.
 */
export function runGroup(nodes: ReplayNode[], edges: ReplayEdge[], entry: ReplayInput): string[] {
  if (entry.kind === "trigger" && entry.route) {
    const action = nodes.find((n) => n.data.name === "action" && n.data.body.kind === "Template" && `action ${frozen(n.data.params, "name") ?? ""}` === entry.route);
    if (!action) return [];
    return components(nodes, edges).find((ids) => ids.includes(action.id)) ?? [];
  }
  if (entry.kind === "file" && entry.route) {
    const onFile = nodes.find((n) => n.data.name === "on_file" && n.data.body.kind === "Template" && fileLabel(n.data.params) === entry.route);
    if (!onFile) return [];
    return components(nodes, edges).find((ids) => ids.includes(onFile.id)) ?? [];
  }
  if (entry.kind === "mail" && entry.route) {
    const onMail = nodes.find((n) => n.data.name === "on_mail" && n.data.body.kind === "Template" && mailLabel(n.data.params) === entry.route);
    if (!onMail) return [];
    return components(nodes, edges).find((ids) => ids.includes(onMail.id)) ?? [];
  }
  if (entry.kind === "feed" && entry.route) {
    const onFeed = nodes.find((n) => n.data.name === "on_feed" && n.data.body.kind === "Template" && feedLabel(n.data.params) === entry.route);
    if (!onFeed) return [];
    return components(nodes, edges).find((ids) => ids.includes(onFeed.id)) ?? [];
  }
  if (entry.kind === "queue" && entry.route) {
    const onQueue = nodes.find((n) => n.data.name === "on_queue" && n.data.body.kind === "Template" && queueLabel(n.data.params) === entry.route);
    if (!onQueue) return [];
    return components(nodes, edges).find((ids) => ids.includes(onQueue.id)) ?? [];
  }
  if (entry.kind === "websocket" && entry.route) {
    const block = nodes.find((n) => n.data.name === "websocket" && n.data.body.kind === "Template" && socketLabel(n.data.params) === entry.route);
    if (!block) return [];
    return components(nodes, edges).find((ids) => ids.includes(block.id)) ?? [];
  }
  if (entry.kind === "tick" && entry.route) {
    const every = nodes.find(
      (n) => n.data.name === "every" && n.data.body.kind === "Template" && `every ${periodText(Number(frozen(n.data.params, "seconds")))}` === entry.route,
    );
    if (!every) return [];
    return components(nodes, edges).find((ids) => ids.includes(every.id)) ?? [];
  }
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

// The leaves that ask the host, read once from core (catalog.rs's ASKS_HOST).
let asksHostLeaves: Set<string> | null = null;
const asksHost = () => (asksHostLeaves ??= asksHostPrimitives());

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

  // `random`'s number from the run's 32 random bytes: their first 53 bits over 2^53, as core's
  // `sourced_value` reads them.
  const randomNumber = (v: WValue | null): WValue | null => {
    if (v?.type !== "Bytes") return null;
    const b = typeof v.value === "string" ? Uint8Array.from(atob(v.value), (c) => c.charCodeAt(0)) : v.value;
    if (b.length < 8) return null;
    const word = (i: number) => ((b[i] << 24) | (b[i + 1] << 16) | (b[i + 2] << 8) | b[i + 3]) >>> 0;
    return { type: "Number", value: (word(0) * 2 ** 21 + (word(4) >>> 11)) / 2 ** 53 };
  };

  // What one node computes once everything it needs has arrived.
  const run = (id: string, node: ReplayNode, inputs: (WValue | null)[]): NodeTrace => {
    const name = node.data.name;
    const primitive = node.data.body.kind === "Primitive";
    const param = (p: string) => frozen(node.data.params, p) ?? "";
    if (primitive && name === "store_read") return { inputs, outputs: [entry.state_before[param("key")] ?? null] };
    if (primitive && name === "clock") return { inputs, outputs: [sourced.get("Clock") ?? null] };
    // A clock's tick: the time it ticked.
    if (node.data.body.kind === "Template" && name === "every") return { inputs, outputs: [sourced.get("Clock") ?? null] };
    // The run's random bytes, as they are, and the number read from them, as core reads it.
    if (primitive && name === "random_bytes") return { inputs, outputs: [sourced.get("Random") ?? null] };
    if (primitive && name === "random") return { inputs, outputs: [randomNumber(sourced.get("Random") ?? null)] };
    // The new file that started the run: its handle, as the log keeps it.
    if (primitive && name === "file_arrival") return { inputs, outputs: [sourced.get(`FileArrival(${JSON.stringify(param("folder"))})`) ?? null] };
    // A file handler's file: its handle, likewise.
    if (node.data.body.kind === "Template" && name === "on_file") return { inputs, outputs: [sourced.get(`FileArrival(${JSON.stringify(param("folder"))})`) ?? null] };
    // The message that started a mail handler's run, as the log keeps it, whole.
    if ((primitive && name === "mail_arrival") || (node.data.body.kind === "Template" && name === "on_mail"))
      return { inputs, outputs: [sourced.get(`MailArrival(${JSON.stringify(param("mail"))})`) ?? null] };
    // The event that started a feed handler's run, as the log keeps it.
    if ((primitive && name === "feed_event") || (node.data.body.kind === "Template" && name === "on_feed"))
      return { inputs, outputs: [sourced.get(`FeedEvent(${JSON.stringify(param("feed"))})`) ?? null] };
    // The message that started a queue handler's run, as the log keeps it.
    if ((primitive && name === "queue_message") || (node.data.body.kind === "Template" && name === "on_queue"))
      return { inputs, outputs: [sourced.get(`QueueMessage(${JSON.stringify(param("queue"))})`) ?? null] };
    // A waiting point answers with the event the log says was delivered, if any: an approval, with
    // the decision.
    if (primitive && (name === "await" || name === "approval")) return { inputs, outputs: [entry.events?.[param("name")] ?? null] };
    // A token is a credential: the log has it redacted, and so does the replay.
    if (primitive && name === "resume_token") return { inputs, outputs: [{ type: "Text", value: `‹token:${param("await")}›` }] };
    // The editor never has a secret's value: a placeholder stands in, as the log shows it.
    if (primitive && name === "secret") {
      const secret = param("name");
      const shown = entry.secrets?.[secret];
      return { inputs, outputs: [{ type: "Text", value: `‹secret:${secret}${shown ? ` ${shown}` : ""}›` }] };
    }
    if (primitive && asksHost().has(name)) {
      // Answered from the record, never the network, the store, the disk or a database: a replay
      // is offline and exact, and shows the collection, folder or table as the run found it,
      // whatever it holds now. Which leaves are core's one list (`catalog::ASKS_HOST`), not one
      // kept here: a list kept here once lacked the database reads, and every run that read a
      // table replayed as though it found nothing.
      return { inputs, outputs: [responses.get(id) ?? null] };
    }
    // A route's request, or a websocket block's record, as the log keeps it.
    if ((name === "request" || name === "websocket") && node.data.body.kind === "Template") {
      const fed = [entry.request];
      return { inputs: fed, outputs: evaluate(node, fed) };
    }
    // An action's: what was sent, as the log keeps it.
    if (name === "action" && node.data.body.kind === "Template") return { inputs: [entry.request], outputs: [entry.request ?? null] };
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
/** What a run gave, as a verdict reads it: its kind, the route or action it ran, what it answered
 *  (nothing, until it answers) and the waiting points it is parked at. */
export type VerdictRun = { kind: string; route: string | null; outputs: (WValue | null)[]; parked?: string[] | null };

/**
 * Whether a replay reproduces the run: true, false, or null when nothing tells.
 *
 * - A request, or an action with a `respond`, that answered: the replay's respond gives the same
 *   answer.
 * - A run parked at a waiting point (an `await`, an approval) hasn't answered: each node it ran
 *   gives what its log says. The replay goes on past the waiting point, which nobody has decided
 *   and so gives nothing, down to a respond the run never reached, which falls back to its
 *   `missing` answer (404 "not found"); held to the answer, every waiting run read as different.
 * - A clock's tick, a file handler's run, an action that only writes: each node gives what the log
 *   says, as a waiting run.
 *
 * And it differs whenever it ran other nodes than the run did (`ranSame`, from `sameNodesRan`).
 */
export function replayVerdict(entry: VerdictRun, run: Pick<Replay, "order" | "traces">, nodes: ReplayNode[], recorded: Map<string, (WValue | null)[]>, ranSame: boolean | null): boolean | null {
  if (ranSame === false) return false;
  const respond = run.order.find((id) => {
    const n = nodes.find((x) => x.id === id);
    return n?.data.name === "respond" && n.data.body.kind === "Template";
  });
  // An action's run is answered as a request is: by its respond.
  const action = entry.kind === "trigger" && !!entry.route;
  const waiting = (entry.parked?.length ?? 0) > 0;
  if (!waiting && (entry.kind === "request" || action) && respond && entry.outputs.length > 0) {
    return JSON.stringify(run.traces.get(respond)?.outputs[0] ?? null) === JSON.stringify(entry.outputs[0]);
  }
  if (waiting || ((entry.kind === "tick" || entry.kind === "file" || (action && !respond)) && entry.route)) {
    const ran = [...recorded].filter(([id]) => run.traces.has(id));
    if (ran.length > 0) return ran.every(([id, outputs]) => stable(run.traces.get(id)!.outputs) === stable(outputs));
  }
  return null;
}

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
    case "file_read":
    case "file_handle":
    case "file_lines":
    case "file_slice":
      return `no file "${textOf(a) ?? "?"}" in the folder "${p("folder")}"`;
    case "find":
    case "find_last":
      return "its function answered true for no item";
    case "at":
      return "no item at that index, or half of an emoji in a text";
    case "with_item":
      return "no item at that index";
    case "slice":
      return "a position it can't use, or the piece would cut an emoji in half";
    case "text_at":
      return "that index is half of an emoji";
    case "code_at":
      return "no character at that index";
    case "from_codes":
      return "a number isn't a character's, or half of an emoji is left alone";
    case "split":
      return "an empty separator can't cut a text with an emoji";
    case "pad":
      return "the fill would be cut through an emoji, or the text would be too long";
    case "repeat":
      return "the count is negative, or the text would be too long";
    case "replace":
      return "the result would cut an emoji in half, or be too long";
    case "uri_decode":
      return "an escape isn't %XX, or isn't valid UTF-8";
    case "base64":
      return "its text isn't base64, or a character is past U+00FF (latin1)";
    case "get":
      return "no item at that index";
    case "head":
    case "tail":
      return "the list is empty";
    case "guard":
      return "its condition was false";
    case "json_parse":
      return "its text isn't valid JSON";
    case "date_part":
    case "date_set":
    case "date_text":
    case "date_utc":
    case "date_add":
      return "the date is out of range (±8.64e15 ms), or isn't one, or its offset isn't +HH:mm, Z or minutes";
    case "business_day_add":
    case "business_days_between":
    case "is_business_day":
      return "a date isn't YYYY-MM-DD or a time value, a holiday isn't a date, or the count is past 100,000";
    case "date_parse":
      return "its text isn't a date in JavaScript's format, with Z or an offset after a time";
    case "from_entries":
      return "an entry isn't a [key, value] pair with a text key";
    case "decimal_add":
    case "decimal_sub":
    case "decimal_mul":
    case "decimal_sum":
    case "decimal_compare":
      return "an input isn't a decimal (a sign, digits, a point), or the result is past 1,000 digits";
    case "decimal_div":
      return "it divides by zero, its scale isn't a whole number from 0 to 1,000, or an input isn't a decimal";
    case "decimal_round":
      return "its scale isn't a whole number from 0 to 1,000, or its input isn't a decimal";
    case "csv_parse":
      return "the CSV is malformed: a quote that never closes, a row with more or fewer fields than the header, or a column named twice";
    case "csv_format":
      return "a field isn't a text, a number or a Boolean, or the rows aren't records with columns given (lists without)";
    case "xml_parse":
      return "the XML isn't well-formed (a tag that doesn't close, an entity other than the five, an attribute given twice), or it has a DOCTYPE";
    case "xml_format":
      return "a name isn't an XML name, a text holds a character XML can't, or an item isn't an element, a text, a number or a Boolean";
    case "xml_find":
    case "xml_text":
    case "xml_attribute":
      return "its input isn't an element (a list starting with a name), a path step isn't a name or *, or nothing matched";
    case "gzip":
      return "its input isn't a text or bytes";
    case "gunzip":
      return "its bytes aren't a whole gzip (a checksum or a length that doesn't match, bytes after it), or they inflate past 64 MiB";
    case "zip":
      return "a file isn't a text or bytes, a name is empty, ends with / or would land outside its folder (a leading /, a \\, a .. part), or there are more than 10,000";
    case "unzip":
      return "its bytes aren't a whole zip, a file is encrypted, zip64 or compressed other than by deflate, a name is given twice or lands outside its folder, or it inflates past 64 MiB or 10,000 files";
    case "jwt_verify":
      return "the token didn't verify with its key (a signature, an algorithm the key doesn't allow, alg none, crit), or its claims don't hold at now: exp passed or missing, nbf ahead, another issuer or audience";
    case "jwt_decode":
      return "its token isn't three base64url parts whose header and payload are JSON objects";
    case "pgp_encrypt":
      return "no key has an encryption key valid at now (not a version 4 RSA or Curve25519 public key, expired, revoked, a subkey its primary didn't bind), or random has fewer than 32 bytes";
    case "pgp_verify":
      return "no signature verified with a key that could sign when it was made (another key, expired or revoked, a signing subkey without its back-signature), it was made after now or has expired, or it is over SHA-1";
    case "pgp_key":
      return "its input isn't one public key (several, a secret key, not OpenPGP), or no self-signature holds for it";
    case "group_by":
      return "its function answered a key that isn't a text";
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
  // A call to another flow's action answers `{answer, run}`, or `{error, message}` when it didn't run.
  const failed = textOf(fieldOf(response, "error"));
  if (failed && fieldOf(response, "status") === null) {
    const message = textOf(fieldOf(response, "message"));
    return `the call failed: ${failed}${message ? ` (${message})` : ""}`;
  }
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
    if ((n.data.name === "http_request" || n.data.name === "http_upload" || n.data.name === "http_stream" || n.data.name === "flow_call" || n.data.name === "flow_start") && n.data.body.kind === "Primitive") {
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
