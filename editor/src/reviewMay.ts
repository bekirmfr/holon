// The Review page's left column (gaps-web.md G9, C4): what a version may do, from a deploy's
// preview (`touches`), grouped as a person decides on it: what needs their approval, what is
// unchanged (or the host allows already), what it no longer does. Each points at the line of the
// flow's text it comes from, found by scanning the text for its name, as the workbench marks
// names (review.rs) — the nearest line, not a proof.

import { touchLines, type SettingChange, type Touch, type TouchLine } from "./touches";

export type MayRow = { line: TouchLine; anchor: number | null };
export type MayGroup = { key: "approve" | "same" | "gone"; title: string; rows: MayRow[] };

/** The text's lines, as the review numbers them. */
export function textLines(text: string): string[] {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** The line (1-based) a thing a flow touches comes from: its name quoted, then its name, then
 *  for the requests it receives its first route, for the clock its first `every`. Comments are
 *  passed over: they say what the code does, the code is where it does it. */
export function anchorOf(t: Pick<Touch, "kind" | "name">, lines: string[]): number | null {
  const first = (test: (l: string) => boolean) => {
    const i = lines.findIndex((l) => !/^\s*(\/\/|\*|\/\*)/.test(l) && test(l));
    return i < 0 ? null : i + 1;
  };
  if (t.name !== "") {
    const parts = [t.name, ...t.name.split("/").filter((p) => p !== t.name)];
    for (const p of parts) {
      const at = first((l) => l.includes(`"${p}"`)) ?? first((l) => l.includes(p));
      if (at !== null) return at;
    }
  }
  if (t.kind === "requests") return first((l) => /\.(get|post|put|patch|delete)\s*\(|\broute\s*\(/i.test(l));
  if (t.kind === "clock") return first((l) => /\bevery\s*\(/.test(l));
  return null;
}

/** Which group a row goes in: a use to approve, a thing no longer touched, or the rest. */
function groupOf(t: Touch): MayGroup["key"] {
  if (t.change === "gone") return "gone";
  return t.uses.some((u) => u.approval && u.change !== "gone") ? "approve" : "same";
}

/** An Approvals card's rows (web-console-L4.md §7.2): what is new and needs approving, what it
 *  no longer does, each with its line, and how many stay the same. */
export type CardRows = { approve: MayRow[]; gone: MayRow[]; same: number };

export function cardRows(groups: MayGroup[]): CardRows {
  const of = (key: MayGroup["key"]) => groups.find((g) => g.key === key)?.rows ?? [];
  return { approve: of("approve"), gone: of("gone"), same: of("same").length };
}

/** What it may do, in the groups a person decides by, each row said as a dry run says it and
 *  anchored to its line; settings that change are with what needs approval. Empty groups are
 *  left out. */
export function mayGroups(touches: Touch[] = [], settings: SettingChange[] = [], lines: string[] = []): MayGroup[] {
  const groups: MayGroup[] = [
    { key: "approve", title: "Needs your approval", rows: [] },
    { key: "same", title: "Unchanged, or the host allows it", rows: [] },
    { key: "gone", title: "No longer", rows: [] },
  ];
  for (const t of touches) {
    const [, line] = touchLines([t]);
    groups.find((g) => g.key === groupOf(t))!.rows.push({ line, anchor: anchorOf(t, lines) });
  }
  for (const line of touchLines([], settings).slice(1)) groups[0].rows.push({ line, anchor: null });
  return groups.filter((g) => g.rows.length > 0);
}

/** A fact the Review states: its name and what it says. */
export type Fact = [string, string];

const ms = (n: number) => (n >= 1000 && n % 1000 === 0 ? `${n / 1000} s` : `${n} ms`);
const live = (t: Touch) => t.change !== "gone";
const named = (ts: Touch[], kind: string, one: string, many: string) => {
  const names = ts.filter((t) => t.kind === kind).map((t) => t.name);
  return names.length === 0 ? [] : [`${names.length === 1 ? one : many} ${names.join(", ")}`];
};

/** While it runs (web-console-L4.md §7.3): its port, its run budget, the runs its log keeps, and
 *  what it holds between runs: its state, and the keys a run holds while it calls out (its
 *  claims, claims.rs). A limit it asks for is its own; the rest are the host's defaults. */
export function whileItRuns({
  port,
  touches = [],
  limits = {},
  defaults = {},
}: {
  /** Its port, as its settings say (none: it has no routes to answer). */
  port: string | null;
  touches?: Touch[];
  /** The limits it asks for. */
  limits?: Record<string, number>;
  /** The host's (GET /permissions); none while unread. */
  defaults?: Record<string, number>;
}): Fact[] {
  const now = touches.filter(live);
  const has = (kind: string) => now.some((t) => t.kind === kind);
  const limit = (name: string) => {
    const own = limits[name];
    const host = defaults[name];
    if (own !== undefined) return { n: own, said: host !== undefined && host !== own ? ` (its own; the host's is ${name.endsWith("_ms") ? ms(host) : host})` : "" };
    return host === undefined ? null : { n: host, said: "" };
  };
  const facts: Fact[] = [];
  const routes = has("requests") || has("websocket");
  facts.push(["port", routes ? (port ? `:${port}` : "the host's default") : "none: nothing calls it over HTTP"]);
  const budgets: string[] = [];
  const request = limit("run_budget_ms");
  if (request && (routes || !has("clock"))) budgets.push(`${ms(request.n)} a run${request.said}`);
  const clock = limit("clock_run_budget_ms");
  if (clock && has("clock")) budgets.push(`${ms(clock.n)} a clock run${clock.said}`);
  facts.push(["run budget", budgets.join("; ") || "the host's"]);
  const runs = limit("keep_runs");
  const days = limit("keep_run_days");
  facts.push(["runs kept", runs ? `the newest ${runs.n}${runs.said}${days ? `, none older than ${days.n} days${days.said}` : ""}` : "as the host keeps them"]);
  const state = now.filter((t) => t.group === "state");
  const kept = [...named(state, "key", "key", "keys"), ...named(state, "collection", "collection", "collections")];
  facts.push(["its state", kept.length > 0 ? kept.join("; ") : "nothing: each run starts afresh"]);
  // A run holds what it read (a key, a row, a file) once it is about to call out, until it commits.
  const read = now.filter((t) => t.group === "state" || t.kind === "database" || t.kind === "folder");
  const calls = now.some((t) => t.group === "outside");
  facts.push([
    "its claims",
    read.length > 0 && calls
      ? "what a run read before it calls out, held by that run until it commits: one run at a time calls out over the same key"
      : "none: no run reads what it keeps and then calls out",
  ]);
  return facts;
}

/** One line of the host's policy a version relies on, and whether it holds. */
export type PolicyRow = { ok: boolean; text: string };

/** The host's policy, as GET /permissions answers it: what the Review checks a version against. */
export type PolicyLines = {
  defaults: Record<string, number>;
  ceilings: Record<string, number>;
  auto: string[];
  allow_plain_http: boolean;
  allow_private_addresses: boolean;
  agents_propose?: boolean;
};

/** Whether the host grants `item` without asking (permissions.rs `is_auto`): `Kind(*)` covers
 *  every item of its kind, any other pattern only itself. Never a tag. */
export function autoGrants(auto: string[], item: string): string | null {
  if (item.startsWith("Tag(")) return null;
  return auto.find((p) => (p.endsWith("(*)") ? item.startsWith(`${p.slice(0, -3)}(`) : p === item)) ?? null;
}

/** Host policy (web-console-L4.md §7.3): a row for each line of the host's policy this version
 *  relies on. Each host it calls is allowed (granted without asking, where `auto` says so), and
 *  plain HTTP and private addresses where it asks; each limit it raises is under its ceiling; what
 *  else the host grants without asking; agents may propose, for an agent's version; and whoever
 *  reviews it may approve it (`may`: `can`'s answer). */
export function policyRows({
  touches = [],
  items = [],
  limits = {},
  privateAddresses = false,
  policy,
  agent,
  may,
  approving,
}: {
  touches?: Touch[];
  /** What it asks for, in all. */
  items?: string[];
  limits?: Record<string, number>;
  privateAddresses?: boolean;
  policy: PolicyLines | null;
  /** Whether an agent proposed it. */
  agent: boolean;
  may: true | string;
  /** Whether deploying it grants something new (flow.approve-deploy), or only deploys (flow.deploy). */
  approving: boolean;
}): PolicyRow[] {
  const rows: PolicyRow[] = [];
  if (policy) {
    const hosts = new Set<string>();
    for (const t of touches.filter((t) => live(t) && t.kind === "host")) {
      for (const u of t.uses.filter((u) => u.change !== "gone")) {
        if (u.item.startsWith("PlainHttp(")) {
          rows.push({ ok: policy.allow_plain_http, text: policy.allow_plain_http ? `plain HTTP to ${t.name}: the host allows it` : `plain HTTP to ${t.name}: the host doesn't allow it` });
          continue;
        }
        hosts.add(u.item);
        const auto = autoGrants(policy.auto, u.item);
        rows.push({ ok: true, text: `it calls ${t.name}: the host allows it${auto ? `, and grants it without asking (auto ${auto})` : ""}` });
      }
    }
    if (privateAddresses) rows.push({ ok: policy.allow_private_addresses, text: policy.allow_private_addresses ? "private addresses: the host allows reaching them" : "private addresses: the host doesn't allow reaching them" });
    for (const [name, n] of Object.entries(limits)) {
      const host = policy.defaults[name];
      if (host === undefined || n <= host) continue;
      const ceiling = policy.ceilings[name];
      rows.push({ ok: ceiling === undefined || n <= ceiling, text: ceiling === undefined ? `${name} = ${n}: above the host's ${host}, with no ceiling` : `${name} = ${n}: above the host's ${host}, ${n <= ceiling ? "within" : "past"} its ceiling of ${ceiling}` });
    }
    for (const item of items.filter((i) => !hosts.has(i))) {
      const auto = autoGrants(policy.auto, item);
      if (auto) rows.push({ ok: true, text: `${item}: the host grants it without asking (auto ${auto})` });
    }
    if (agent) rows.push({ ok: policy.agents_propose !== false, text: policy.agents_propose !== false ? "agents may propose versions (agents.propose)" : "agents may not propose versions (agents.propose = false)" });
  }
  const action = approving ? "flow.approve-deploy" : "flow.deploy";
  rows.push(may === true ? { ok: true, text: approving ? `you may approve what it asks for (${action})` : `you may deploy it (${action})` } : { ok: false, text: may });
  return rows;
}
