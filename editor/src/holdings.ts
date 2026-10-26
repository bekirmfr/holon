// Who holds what, for the admin page's permissions (docs/L4s/databases-L4.md §9.1): each
// deployment's grant (`GET /admin/grants`) turned round, by the folder or database each item
// reaches, through the binding that deployment sees: its own binding of the name, else the
// server's (folders.rs's and databases.rs's `resolve`).

import type { HeldGrant } from "./server";

/** A folder's or database's binding: its name, for the server (`deployment` null) or one deployment. */
export type Binding = { name: string; deployment: string | null };

/** A grant item's kind and argument: `DbWrite("shop/orders")` is `DbWrite` of `shop/orders`;
 *  `Clock` and `Limit(max_requests=80)` have none. */
export function itemParts(item: string): { kind: string; arg: string | null } {
  const quoted = /^([A-Za-z]+)\("((?:[^"\\]|\\.)*)"\)$/.exec(item);
  if (quoted) return { kind: quoted[1], arg: quoted[2].replace(/\\(.)/g, "$1") };
  return { kind: /^[A-Za-z]+/.exec(item)?.[0] ?? item, arg: null };
}

/** What an item reaches, when it reaches a folder or a database: which, by name, how, and (a
 *  database's) which table. */
export function reaches(item: string): { kind: "folder" | "database"; name: string; use: Use["use"]; table: string | null } | null {
  const { kind, arg } = itemParts(item);
  if (arg === null) return null;
  const folder = { FileRead: "read", FileWrite: "write", FileArrival: "arrival" } as const;
  if (kind in folder) return { kind: "folder", name: arg, use: folder[kind as keyof typeof folder], table: null };
  if (kind === "DbRead" || kind === "DbWrite") {
    const slash = arg.indexOf("/");
    const [name, table] = slash < 0 ? [arg, null] : [arg.slice(0, slash), arg.slice(slash + 1)];
    return { kind: "database", name, use: kind === "DbRead" ? "read" : "write", table };
  }
  return null;
}

/** The binding deployment `slug` sees as `name`: its own, else the server's. */
export function seenBy<B extends Binding>(bindings: B[], name: string, slug: string): B | undefined {
  return bindings.find((b) => b.name === name && b.deployment === slug) ?? bindings.find((b) => b.name === name && b.deployment === null);
}

/** One way a deployment reaches a resource: reads it, writes it, starts on its new files; a
 *  database's by table. `revoked`: the host took it back since. */
export type Use = { use: "read" | "write" | "arrival"; table: string | null; revoked: boolean; item: string };
export type Holder = Pick<HeldGrant, "slug" | "status" | "attention"> & { uses: Use[] };

const same = (a: Binding, b: Binding) => a.name === b.name && a.deployment === b.deployment;

/** The deployments that reach `binding` (a `kind`), and how: those whose items name it and that
 *  see this binding by its name, revoked items among them. */
export function holdersOf(kind: "folder" | "database", binding: Binding, bindings: Binding[], held: HeldGrant[]): Holder[] {
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

/** Every other item held, by item, with who holds it: a host called, a secret read, a state
 *  key, and a folder or database whose name no binding answers. Limits are left out: they bound
 *  a run, they reach nothing. */
export function othersHeld(held: HeldGrant[], folders: Binding[], databases: Binding[]): { item: string; kind: string; holders: { slug: string; revoked: boolean }[] }[] {
  const byItem = new Map<string, { slug: string; revoked: boolean }[]>();
  for (const d of held) {
    for (const [items, revoked] of [
      [d.items, false],
      [d.revoked, true],
    ] as const) {
      for (const item of items) {
        if (itemParts(item).kind === "Limit") continue;
        const r = reaches(item);
        if (r && seenBy(r.kind === "folder" ? folders : databases, r.name, d.slug)) continue;
        byItem.set(item, [...(byItem.get(item) ?? []), { slug: d.slug, revoked }]);
      }
    }
  }
  return [...byItem.entries()].sort(([a], [b]) => a.localeCompare(b)).map(([item, holders]) => ({ item, kind: kindOf(item), holders }));
}

/** What the permissions view filters by: `databases` for DbRead and DbWrite, `folders` for the
 *  file kinds, else the item's own kind (`HttpRequest`, `SecretRead`, …). */
export function kindOf(item: string): string {
  const r = reaches(item);
  if (r) return r.kind === "folder" ? "folders" : "databases";
  return itemParts(item).kind;
}

/** The kinds held across every grant, databases and folders first, then the rest by name. */
export function kindsHeld(held: HeldGrant[]): string[] {
  const kinds = new Set(held.flatMap((d) => [...d.items, ...d.revoked]).filter((i) => itemParts(i).kind !== "Limit").map(kindOf));
  const first = ["databases", "folders"].filter((k) => kinds.has(k));
  return [...first, ...[...kinds].filter((k) => !first.includes(k)).sort()];
}

/** How a holder reaches it, as the page says it: `reads orders, refunds`, `writes orders`,
 *  `starts on new files`; a folder's without tables. Each with whether it was revoked. */
export function usesText(uses: Use[]): { text: string; revoked: boolean }[] {
  const verb = { read: "reads", write: "writes", arrival: "starts on new files" } as const;
  const out: { text: string; revoked: boolean }[] = [];
  for (const revoked of [false, true]) {
    for (const use of ["read", "write", "arrival"] as const) {
      const these = uses.filter((u) => u.use === use && u.revoked === revoked);
      if (these.length === 0) continue;
      const tables = [...new Set(these.map((u) => u.table).filter((t): t is string => t !== null))].sort();
      out.push({ text: tables.length > 0 ? `${verb[use]} ${tables.join(", ")}` : verb[use], revoked });
    }
  }
  return out;
}
