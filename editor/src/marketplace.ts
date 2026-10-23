// The marketplace, as the editor uses it: a Holon flow (the "marketplace" sample) deployed
// somewhere, answering web pages. A listing is a holon as published: its name, description,
// ports, manifest, and its units by hash (hex preimages). Publishing takes the marketplace's
// publish key; reading is open. Placing a listing puts its units into the workspace, each
// re-hashed on insert, and refuses it unless its root is among them: the hash is the proof, so a
// marketplace can describe a holon but can't swap one. Trust is adoption (how many other listings
// place it) and a yank flag, never an edit (docs/COMPOSABLE_VM_DESIGN.md §8).

import { flowAsNode } from "./canvasOps";
import { documentUnits } from "./docUnits";
import { fromHex } from "./flowDoc";
import type { Evaluator, WPort, WUnit } from "./holon";
import type { PrimitiveNodeType } from "./PrimitiveNode";

/** A holon as published. `contains`: every unit in its closure, by hash (for adoption). */
export type Listing = {
  id: string;
  name: string;
  description: string;
  root: string;
  units: string[];
  contains: string[];
  inputs: WPort[];
  outputs: WPort[];
  manifest: string[];
  publisher: string;
  published_at: number;
  yanked?: boolean;
};

/** A dragged marketplace listing's payload: `market:<id>`. */
export const MARKET_PREFIX = "market:";

const URL_KEY = "holon.marketplace";
const KEYS_KEY = "holon.marketplace.keys";

const read = <T>(key: string, fallback: T): T => {
  try {
    return (JSON.parse(localStorage.getItem(key) ?? "null") as T | null) ?? fallback;
  } catch {
    return fallback;
  }
};
const write = (key: string, value: unknown) => {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // storage unavailable: kept for this page only
  }
};

/** The marketplace this browser uses (its deployment's address), or "". Kept in this browser. */
export const marketplaceUrl = (): string => read(URL_KEY, "");
export const setMarketplaceUrl = (url: string) => write(URL_KEY, url.trim().replace(/\/+$/, ""));

/** The publish key kept for `url`, or "". */
export const publishKeyFor = (url: string): string => read<Record<string, string>>(KEYS_KEY, {})[url] ?? "";
export const setPublishKey = (url: string, key: string) => write(KEYS_KEY, { ...read<Record<string, string>>(KEYS_KEY, {}), [url]: key });

async function asJson<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const body = (await res.json().catch(() => null)) as { error?: string } | null;
    throw new Error(body?.error ?? `the marketplace answered ${res.status}`);
  }
  return (await res.json()) as T;
}

/** Every listing, newest first. */
export async function listListings(url: string): Promise<Listing[]> {
  const all = await asJson<Record<string, Listing>>(await fetch(`${url}/listings`));
  return Object.values(all).sort((a, b) => b.published_at - a.published_at);
}

export async function publishListing(url: string, key: string, listing: Omit<Listing, "id" | "published_at">): Promise<Listing> {
  return asJson<Listing>(
    await fetch(`${url}/listings`, { method: "POST", headers: { "Content-Type": "application/json", Authorization: `Bearer ${key}` }, body: JSON.stringify(listing) }),
  );
}

export async function yankListing(url: string, key: string, id: string): Promise<Listing> {
  return asJson<Listing>(await fetch(`${url}/listings/${encodeURIComponent(id)}/yank`, { method: "POST", headers: { Authorization: `Bearer ${key}` } }));
}

/** A saved flow as a listing: placed as one node (flowAsNode), its closure's units, its manifest
 *  from core. Why not, when it can't be placed as a node (a flow with routes, say). */
export function listingOf(
  name: string,
  doc: { nodes: unknown[]; edges: unknown[]; units?: string[]; names?: Record<string, string> },
  ev: Evaluator,
  about: { description: string; publisher: string },
): Omit<Listing, "id" | "published_at"> | string {
  const placed = flowAsNode(name, doc, ev, "listing", { x: 0, y: 0 });
  if (!placed.ok) return placed.message;
  const body = placed.node.data.body;
  if (body.kind !== "Composite") return `"${name}" isn't a composite`;
  const { units } = documentUnits([body.hash], ev);
  const contains = units.map((hex) => ev.insert(fromHex(hex)));
  return {
    name,
    description: about.description,
    root: body.hash,
    units,
    contains,
    inputs: placed.node.data.inputs,
    outputs: placed.node.data.outputs,
    manifest: (ev.capability_names(body.hash) as string[]).sort(),
    publisher: about.publisher,
  };
}

/** A listing's units put into the workspace, and its node at `position`: refused unless its
 *  root is among the units as re-hashed, so what is placed is what the hash names. */
export function placeListing(listing: Listing, ev: Evaluator, id: string, position: { x: number; y: number }): PrimitiveNodeType | string {
  let hashes: string[];
  try {
    hashes = listing.units.map((hex) => ev.insert(fromHex(hex)));
  } catch (e) {
    return `"${listing.name}": its units don't decode: ${e instanceof Error ? e.message : String(e)}`;
  }
  if (!hashes.includes(listing.root)) return `"${listing.name}": its units don't hash to its root ${listing.root.slice(0, 12)}…: not placed`;
  const unit = ev.get_unit(listing.root) as WUnit;
  return {
    id,
    type: "primitive",
    position,
    data: {
      name: listing.name,
      body: { kind: "Composite", hash: listing.root },
      // The unit's own ports, named as the listing names them (the publisher's node's names).
      inputs: (unit.inputs ?? []).map((p, i) => ({ ...p, name: listing.inputs[i]?.name ?? p.name })),
      outputs: (unit.outputs ?? []).map((p, i) => ({ ...p, name: listing.outputs[i]?.name ?? p.name })),
      params: [],
      variadic: false,
      minInputs: 0,
    },
  };
}

/** Per name, its newest listing that isn't yanked (else its newest), with how many other
 *  listings' names place it (adoption), newest first. */
export function catalogOf(listings: Listing[]): { listing: Listing; adoption: number; versions: number }[] {
  const byName = new Map<string, Listing[]>();
  for (const l of listings) byName.set(l.name, [...(byName.get(l.name) ?? []), l]);
  const adoptersOf = (root: string, self: string) => new Set(listings.filter((l) => l.name !== self && l.contains.includes(root)).map((l) => l.name)).size;
  return [...byName.values()]
    .map((versions) => {
      const sorted = [...versions].sort((a, b) => b.published_at - a.published_at);
      const listing = sorted.find((l) => !l.yanked) ?? sorted[0];
      return { listing, adoption: adoptersOf(listing.root, listing.name), versions: versions.length };
    })
    .sort((a, b) => b.listing.published_at - a.listing.published_at);
}
