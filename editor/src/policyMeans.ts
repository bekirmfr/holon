// What the host's policy means, in words (web-console-L4.md §9.3): one sentence for each limit
// (holon-author's LIMIT_NAMES), written once, for Policy's limits table and a flow's Settings; and
// a waiting version checked against the policy as it is now, item by item, as the server's
// `refuses_item` and `is_auto` (permissions.rs) would take it at its deploy.

import type { HostPolicy, Proposal } from "./server";

/** Each limit a flow may ask for, and what it holds a run to, in one sentence. */
export const LIMIT_MEANINGS: Record<string, string> = {
  connect_timeout_ms: "How long a call out may take to connect to its host.",
  request_timeout_ms: "How long a call out may take, from connecting to the last byte of its answer.",
  claim_wait_ms: "How long a run may wait, in all, for keys other runs hold, before it is turned away.",
  commit_wait_ms: "How long a run's commit may wait for the locks other runs' commits hold on what it read or writes.",
  run_budget_ms: "How long a run someone waits on may take, every call out included.",
  clock_run_budget_ms: "How long a run nobody waits on (a clock tick, a trigger) may take.",
  max_requests: "The calls out one run may make.",
  per_host_concurrency: "The calls out to one host a run has in flight at once.",
  max_request_bytes: "The largest body a call out sends.",
  max_response_bytes: "The largest answer a call out takes in.",
  max_header_bytes: "The largest headers an answer to a call out may have.",
  max_redirects: "The redirects a call out follows.",
  max_steps: "The steps a run may take between two of its calls out: each node it evaluates, each item a function is applied to.",
  max_fuel: "What a run's modules may compute in all between two of its calls out.",
  max_memory: "The memory a run may come to hold between two of its calls out, in bytes.",
  max_await_seconds: "How long one waiting point (an approval, an event) may wait.",
  max_waiting_runs: "The runs of one deployment that may wait at once.",
  max_call_depth: "How deep a chain of flows calling flows may go.",
  inline_file_bytes: "The largest file a run reads whole, and the largest range it reads of one.",
  max_file_read_bytes: "What a run reads from files, in all.",
  max_file_write_bytes: "What a run writes and copies into files, in all.",
  max_files_per_run: "The files a run reads, takes a handle to, writes, copies or deletes.",
  max_served_bytes: "The largest file a route answers with.",
  max_db_rows: "The rows a run's queries give, in all.",
  max_db_read_bytes: "The bytes a run's queries give, in all.",
  db_statement_ms: "How long one database statement may take.",
  max_db_statements: "The statements a run sends its databases: queries and writes.",
  max_pack_bytes: "The largest pack a run sends on a channel.",
  max_packs_per_run: "The packs one run may send.",
  channel_keep: "The packs a channel keeps for a subscriber that asks for them, or catches up.",
  channel_buffer: "The packs waiting for one subscriber before it is dropped as too slow.",
  max_subscribers: "The subscribers one channel has at once.",
  max_websockets: "The WebSocket connections a deployment holds open at once.",
  max_message_bytes: "The largest message a WebSocket connection may send.",
  messages_per_second: "The messages a WebSocket connection may send a second, in bursts of twice that.",
  keep_runs: "The newest runs a deployment's log keeps.",
  keep_run_days: "How old a run its log keeps may be, in days.",
  module_fuel: "What a flow's module may compute in one call.",
  module_memory: "The memory a flow's module may hold in one call, in bytes.",
};

/** What a limit means, or nothing for one this editor doesn't know. */
export const limitMeaning = (name: string): string | undefined => LIMIT_MEANINGS[name];

/** What the policy says to one item a version asks for: it refuses it, grants it without asking
 *  (`auto`, or a limit lowered), or leaves it to a person. `why` ends the sentence "refunds v4
 *  would call https://api.stripe.com, which …". */
export type Verdict = { item: string; says: "refused" | "auto" | "person"; why: string };

const limitOf = (item: string): [string, number] | null => {
  const m = /^Limit\(([a-z_]+)=(\d+)\)$/.exec(item);
  return m ? [m[1], Number(m[2])] : null;
};

/** `item` against `policy`, as the server takes it (permissions.rs `refuses_item`, `is_auto`,
 *  `needs_approval`). */
export function checkItem(item: string, policy: HostPolicy): Verdict {
  const limit = limitOf(item);
  if (limit) {
    const [name, n] = limit;
    const ceiling = policy.ceilings[name];
    if (ceiling === undefined) return { item, says: "refused", why: `the policy refuses: it knows no limit ${name}` };
    if (n > ceiling) return { item, says: "refused", why: `the policy refuses: its ceiling for ${name} is ${ceiling}` };
    const def = policy.defaults[name];
    if (def !== undefined && n <= def) return { item, says: "auto", why: `the policy grants without asking: it is at or under the default, ${def}` };
    return { item, says: "person", why: `the policy allows up to its ceiling of ${ceiling}: a person approves it` };
  }
  if (item.startsWith("PlainHttp(") && !policy.allow_plain_http) return { item, says: "refused", why: "the policy refuses: grants.allow_plain_http is off" };
  if (item === "PrivateAddresses" && !policy.allow_private_addresses) return { item, says: "refused", why: "the policy refuses: grants.allow_private_addresses is off" };
  if (item.startsWith("Unrecorded(") && policy.allow_unrecorded === false) return { item, says: "refused", why: "the policy refuses: grants.allow_unrecorded is off" };
  // A tag is never granted by the policy: it says what a flow may reach, not who may act on it.
  const auto = !item.startsWith("Tag(") && policy.auto.find((p) => (p.endsWith("(*)") ? item.startsWith(`${p.slice(0, -3)}(`) : p === item));
  if (auto) return { item, says: "auto", why: `the policy grants without asking (grants.auto has ${auto})` };
  return { item, says: "person", why: "nothing in the policy refuses: a person approves it" };
}

/** A version waiting for a person, checked: its flow, the number it would be, and each item it
 *  asks to be approved against the policy now. */
export type WaitingCheck = { slug: string; version: number | null; id: number; verdicts: Verdict[] };

/** The waiting proposals, each checked against `policy`: what each asks to be approved (`needs`),
 *  the policy as it is now, which may have changed since it was made. */
export function checkWaiting(proposals: Proposal[], policy: HostPolicy): WaitingCheck[] {
  return proposals
    .filter((p) => p.state === "waiting")
    .map((p) => ({ slug: p.slug, version: p.version ?? null, id: p.id, verdicts: p.needs.map((i) => checkItem(i, policy)) }));
}

/** A waiting version's name as the pages say it: `refunds v4`, or `refunds's waiting version`. */
export const versionName = (w: { slug: string; version: number | null }) => (w.version !== null ? `${w.slug} v${w.version}` : `${w.slug}'s waiting version`);
