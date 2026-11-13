// Who holds what, for the admin page's permissions (docs/L4s/databases-L4.md §9.1): each
// deployment's grant (`GET /admin/grants`) turned round, by the folder, database or mail each
// item reaches, through the binding that deployment sees: its own binding of the name, else the
// server's (folders.rs's, databases.rs's, mails.rs's, feeds.rs's and queues.rs's `resolve`).

import type { HeldGrant, Proposal } from "./server";

/** A folder's, database's, mail's, feed's or queue's binding: its name, for the server (`deployment` null) or one deployment. */
export type Binding = { name: string; deployment: string | null };

/** A grant item's kind and argument: `DbWrite("shop/orders")` is `DbWrite` of `shop/orders`;
 *  `Clock` and `Limit(max_requests=80)` have none. */
export function itemParts(item: string): { kind: string; arg: string | null } {
  const quoted = /^([A-Za-z]+)\("((?:[^"\\]|\\.)*)"\)$/.exec(item);
  if (quoted) return { kind: quoted[1], arg: quoted[2].replace(/\\(.)/g, "$1") };
  return { kind: /^[A-Za-z]+/.exec(item)?.[0] ?? item, arg: null };
}

/** The resources an item can reach through a binding. */
export type Kind = "folder" | "database" | "mail" | "feed" | "queue";

/** What an item reaches, when it reaches a folder, a database, a mail, a feed or a queue: which, by name, how,
 *  and a database's table or a mail's recipient (`*`: any its binding allows). */
export function reaches(item: string): { kind: Kind; name: string; use: Use["use"]; table: string | null } | null {
  const { kind, arg } = itemParts(item);
  if (arg === null) return null;
  const folder = { FileRead: "read", FileWrite: "write", FileArrival: "arrival", FileServe: "serve" } as const;
  if (kind in folder) return { kind: "folder", name: arg, use: folder[kind as keyof typeof folder], table: null };
  if (kind === "DbRead" || kind === "DbWrite") {
    const slash = arg.indexOf("/");
    const [name, table] = slash < 0 ? [arg, null] : [arg.slice(0, slash), arg.slice(slash + 1)];
    return { kind: "database", name, use: kind === "DbRead" ? "read" : "write", table };
  }
  if (kind === "MailArrival") return { kind: "mail", name: arg, use: "receive", table: null };
  if (kind === "FeedEvent") return { kind: "feed", name: arg, use: "listen", table: null };
  if (kind === "QueueMessage") return { kind: "queue", name: arg, use: "dequeue", table: null };
  if (kind === "QueueSend") return { kind: "queue", name: arg, use: "enqueue", table: null };
  if (kind === "MailSend") {
    const slash = arg.indexOf("/");
    return slash < 0 ? { kind: "mail", name: arg, use: "send", table: null } : { kind: "mail", name: arg.slice(0, slash), use: "send", table: arg.slice(slash + 1) };
  }
  return null;
}

/** The binding deployment `slug` sees as `name`: its own, else the server's. */
export function seenBy<B extends Binding>(bindings: B[], name: string, slug: string): B | undefined {
  return bindings.find((b) => b.name === name && b.deployment === slug) ?? bindings.find((b) => b.name === name && b.deployment === null);
}

/** One way a deployment reaches a resource: reads it, writes it, starts on its new files, answers
 *  its callers with its files, mails through it, starts on mail it receives or on a feed's events, starts on a queue's messages or
 *  sends to it; a database's by table, a mail's by recipient. `revoked`: the host took it back
 *  since. */
export type Use = { use: "read" | "write" | "arrival" | "serve" | "send" | "receive" | "listen" | "dequeue" | "enqueue"; table: string | null; revoked: boolean; item: string };
export type Holder = Pick<HeldGrant, "slug" | "status" | "attention"> & { uses: Use[] };

const same = (a: Binding, b: Binding) => a.name === b.name && a.deployment === b.deployment;

/** The deployments that reach `binding` (a `kind`), and how: those whose items name it and that
 *  see this binding by its name, revoked items among them. */
export function holdersOf(kind: Kind, binding: Binding, bindings: Binding[], held: HeldGrant[]): Holder[] {
  const holders: Holder[] = [];
  for (const d of held) {
    const uses: Use[] = [];
    for (const [items, revoked] of [
      [d.items, false],
      [d.revoked, true],
    ] as const) {
      for (const item of items) {
        const r = reaches(item);
        if (!r || r.kind !== kind || r.name !== binding.name) continue;
        const seen = seenBy(bindings, r.name, d.slug);
        if (seen && same(seen, binding)) uses.push({ use: r.use, table: r.table, revoked, item });
      }
    }
    if (uses.length > 0) holders.push({ slug: d.slug, status: d.status, attention: d.attention, uses });
  }
  return holders;
}

/** A version waiting for a person (a proposal) that would reach a resource: its flow, the number it
 *  would be, and what it would add (web-console-L4.md §9.1), read from what it asks to be approved. */
export type Waiting = { slug: string; version: number | null; uses: Use[] };

/** The waiting versions that would reach `binding` (a `kind`), and how: each proposal's items that
 *  name it, through the binding its flow would see by that name. What it holds already isn't
 *  among them: a proposal asks only for what is new. */
export function waitingFor(kind: Kind, binding: Binding, bindings: Binding[], proposals: Proposal[]): Waiting[] {
  const out: Waiting[] = [];
  for (const p of proposals) {
    if (p.state !== "waiting") continue;
    const uses: Use[] = [];
    for (const item of p.needs) {
      const r = reaches(item);
      if (!r || r.kind !== kind || r.name !== binding.name) continue;
      const seen = seenBy(bindings, r.name, p.slug);
      if (seen && same(seen, binding)) uses.push({ use: r.use, table: r.table, revoked: false, item });
    }
    if (uses.length > 0) out.push({ slug: p.slug, version: p.version ?? null, uses });
  }
  return out;
}

/** The deployments whose grants read the secret `name`, and the waiting versions that would where
 *  theirs doesn't now (an agent's version asks again for a secret its flow reads: agents' deploys
 *  leave secrets out, for a person to grant back). */
export function secretReaders(name: string, held: HeldGrant[], proposals: Proposal[]): { now: string[]; waiting: { slug: string; version: number | null }[] } {
  const reads = (items: string[]) => items.some((i) => {
    const { kind, arg } = itemParts(i);
    return kind === "SecretRead" && arg === name;
  });
  const now = held.filter((d) => reads(d.items)).map((d) => d.slug);
  return { now, waiting: proposals.filter((p) => p.state === "waiting" && reads(p.needs) && !now.includes(p.slug)).map((p) => ({ slug: p.slug, version: p.version ?? null })) };
}

/** A database's matrix (§9.1): its tables down, the flows that use it across, and in each cell
 *  what a flow does with that table now and what a waiting version would add. A table no flow
 *  names is a row of its own, empty; a use of no table in particular (`DbRead("shop")`) is the
 *  row `null`. */
export type MatrixCell = { now: ("read" | "write")[]; waiting: ("read" | "write")[]; revoked: boolean };
export type Matrix = { flows: { slug: string; version: number | null; waiting: boolean }[]; rows: { table: string | null; cells: MatrixCell[] }[] };

export function databaseMatrix(tables: string[], holders: Holder[], waiting: Waiting[]): Matrix {
  const flows = [
    ...holders.map((h) => ({ slug: h.slug, version: null as number | null, waiting: false })),
    ...waiting.filter((w) => !holders.some((h) => h.slug === w.slug)).map((w) => ({ slug: w.slug, version: w.version, waiting: true })),
  ];
  for (const f of flows) f.version = waiting.find((w) => w.slug === f.slug)?.version ?? f.version;
  const named = [...holders.flatMap((h) => h.uses), ...waiting.flatMap((w) => w.uses)].map((u) => u.table);
  const all: (string | null)[] = [...new Set([...tables, ...named.filter((t): t is string => t !== null)])].sort();
  if (named.includes(null)) all.unshift(null);
  const verbs = (uses: Use[], table: string | null) => (["read", "write"] as const).filter((v) => uses.some((u) => u.table === table && u.use === v));
  const rows = all.map((table) => ({
    table,
    cells: flows.map((f) => {
      const uses = holders.find((h) => h.slug === f.slug)?.uses ?? [];
      const live = uses.filter((u) => !u.revoked);
      const now = verbs(live, table);
      const would = verbs(waiting.find((w) => w.slug === f.slug)?.uses ?? [], table).filter((v) => !now.includes(v));
      return { now, waiting: would, revoked: now.length === 0 && verbs(uses, table).length > 0 };
    }),
  }));
  return { flows, rows };
}

/** Every other item held, by item, with who holds it: a host called, a secret read, a state
 *  key, and a folder, database, mail, feed or queue whose name no binding answers. Limits are left
 *  out: they bound a run, they reach nothing. */
export function othersHeld(
  held: HeldGrant[],
  folders: Binding[],
  databases: Binding[],
  mails: Binding[] = [],
  feeds: Binding[] = [],
  queues: Binding[] = [],
): { item: string; kind: string; holders: { slug: string; revoked: boolean }[] }[] {
  const byItem = new Map<string, { slug: string; revoked: boolean }[]>();
  for (const d of held) {
    for (const [items, revoked] of [
      [d.items, false],
      [d.revoked, true],
    ] as const) {
      for (const item of items) {
        if (itemParts(item).kind === "Limit") continue;
        const r = reaches(item);
        if (r && seenBy({ folder: folders, database: databases, mail: mails, feed: feeds, queue: queues }[r.kind], r.name, d.slug)) continue;
        byItem.set(item, [...(byItem.get(item) ?? []), { slug: d.slug, revoked }]);
      }
    }
  }
  return [...byItem.entries()].sort(([a], [b]) => a.localeCompare(b)).map(([item, holders]) => ({ item, kind: kindOf(item), holders }));
}

/** What the permissions view filters by: `databases` for DbRead and DbWrite, `folders` for the
 *  file kinds, `mail` for MailSend and MailArrival, `feeds` for FeedEvent, `queues` for QueueMessage and QueueSend, else the
 *  item's own kind (`HttpRequest`, `SecretRead`, …). */
export function kindOf(item: string): string {
  const r = reaches(item);
  if (r) return { folder: "folders", database: "databases", mail: "mail", feed: "feeds", queue: "queues" }[r.kind];
  return itemParts(item).kind;
}

/** The kinds held across every grant, databases, folders, mail, feeds and queues first, then the
 *  rest by name. */
export function kindsHeld(held: HeldGrant[]): string[] {
  const kinds = new Set(held.flatMap((d) => [...d.items, ...d.revoked]).filter((i) => itemParts(i).kind !== "Limit").map(kindOf));
  const first = ["databases", "folders", "mail", "feeds", "queues"].filter((k) => kinds.has(k));
  return [...first, ...[...kinds].filter((k) => !first.includes(k)).sort()];
}

/** How a holder reaches it, as the page says it: `reads orders, refunds`, `writes orders`,
 *  `starts on new files`, `serves its files`, `mails approvals@acme.com, addresses it computes`, `starts on mail it
 *  receives`, `starts on each event it sends`, `starts on each message it holds`, `sends messages to
 *  it`; a folder's without tables. Each with whether it was revoked. */
export function usesText(uses: Use[]): { text: string; revoked: boolean }[] {
  const verb = {
    read: "reads",
    write: "writes",
    arrival: "starts on new files",
    serve: "serves its files",
    send: "mails",
    receive: "starts on mail it receives",
    listen: "starts on each event it sends",
    dequeue: "starts on each message it holds",
    enqueue: "sends messages to it",
  } as const;
  const out: { text: string; revoked: boolean }[] = [];
  for (const revoked of [false, true]) {
    for (const use of ["read", "write", "arrival", "serve", "send", "receive", "listen", "dequeue", "enqueue"] as const) {
      const these = uses.filter((u) => u.use === use && u.revoked === revoked);
      if (these.length === 0) continue;
      const named = [...new Set(these.map((u) => u.table).filter((t): t is string => t !== null))].sort();
      // A mail's computed recipients, last.
      const tables = use === "send" && named.includes("*") ? [...named.filter((t) => t !== "*"), "addresses it computes"] : named;
      out.push({ text: tables.length > 0 ? `${verb[use]} ${tables.join(", ")}` : verb[use], revoked });
    }
  }
  return out;
}
