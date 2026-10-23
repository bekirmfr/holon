// The marketplace as the editor drives it (marketplace.ts): its listings, read when the canvas
// shows and after publishing; publishing a saved flow (asking for a description, who publishes,
// and the publish key the first time); and placing a listing, after saying what it may do.

import { useCallback, useEffect, useState } from "react";
import { catalogOf, listListings, listingOf, marketplaceUrl, placeListing, publishKeyFor, publishListing, setPublishKey, yankListing, type Listing } from "./marketplace";
import { getFlow } from "./server";
import { signedIn } from "./signin";
import { describeItem, type Evaluator } from "./holon";
import type { PrimitiveNodeType } from "./PrimitiveNode";

type Dialogs = { confirm: (message: string, action?: string) => Promise<boolean>; prompt: (message: string, initial?: string) => Promise<string | null> };

export function useMarketplace({ refreshOn, ev, dialog }: { refreshOn: unknown; ev: Evaluator; dialog: Dialogs }) {
  const [url, setUrl] = useState(marketplaceUrl);
  const [listings, setListings] = useState<Listing[]>([]);
  const [error, setError] = useState("");
  const refresh = useCallback(() => {
    const at = marketplaceUrl();
    setUrl(at);
    if (!at) return setListings([]);
    listListings(at)
      .then((l) => {
        setListings(l);
        setError("");
      })
      .catch((e: unknown) => setError(`marketplace: ${e instanceof Error ? e.message : String(e)}`));
  }, []);
  useEffect(() => {
    if (refreshOn !== null) refresh();
  }, [refreshOn, refresh]);

  /** Publish the saved flow `name`; what happened, said. */
  const publish = useCallback(
    async (name: string): Promise<string> => {
      const at = marketplaceUrl();
      if (!at) return "set a marketplace first";
      const description = await dialog.prompt(`Publish "${name}" to ${at}. What does it do (one line)?`);
      if (!description) return "";
      const publisher = signedIn()?.who ?? (await dialog.prompt("Publish as:"));
      if (!publisher) return "";
      let key = publishKeyFor(at);
      if (!key) {
        key = (await dialog.prompt(`The marketplace's publish key (kept in this browser for ${at}):`)) ?? "";
        if (!key) return "";
      }
      const listing = listingOf(name, (await getFlow(name)).document, ev, { description, publisher });
      if (typeof listing === "string") return `not published: ${listing}`;
      try {
        const done = await publishListing(at, key, listing);
        setPublishKey(at, key);
        refresh();
        return `published "${name}" (${done.root.slice(0, 8)}) to the marketplace`;
      } catch (e) {
        return `not published: ${e instanceof Error ? e.message : String(e)}`;
      }
    },
    [ev, dialog, refresh],
  );

  /** The listing `id` as a node at `position`, once the person has seen what it may do; null
   *  when they decline, or why it can't be placed. */
  const place = useCallback(
    async (id: string, nodeId: string, position: { x: number; y: number }): Promise<PrimitiveNodeType | string | null> => {
      const listing = listings.find((l) => l.id === id);
      if (!listing) return "that listing is gone from the marketplace";
      const lines = [`Place "${listing.name}" from the marketplace?`, "", listing.description, `published by ${listing.publisher}`];
      if (listing.yanked) lines.push("", "This version was yanked by its publisher: use it only if you know why.");
      lines.push("", listing.manifest.length > 0 ? "It may:" : "It only computes: it reads and changes nothing outside.", ...listing.manifest.map((m) => `• ${describeItem(m)}`));
      if (!(await dialog.confirm(lines.join("\n"), "Place"))) return null;
      return placeListing(listing, ev, nodeId, position);
    },
    [listings, ev, dialog],
  );

  /** Yank the listing `id` (with the publish key), after asking; what happened, said. */
  const yank = useCallback(
    async (id: string): Promise<string> => {
      const at = marketplaceUrl();
      const listing = listings.find((l) => l.id === id);
      if (!at || !listing) return "";
      if (!(await dialog.confirm(`Yank "${listing.name}" (${listing.root.slice(0, 8)})? It stays listed, flagged: flows that placed it keep it, and are told.`, "Yank"))) return "";
      const key = publishKeyFor(at) || ((await dialog.prompt(`The marketplace's publish key (kept in this browser for ${at}):`)) ?? "");
      if (!key) return "";
      try {
        await yankListing(at, key, id);
        setPublishKey(at, key);
        refresh();
        return `yanked "${listing.name}" (${listing.root.slice(0, 8)})`;
      } catch (e) {
        return `not yanked: ${e instanceof Error ? e.message : String(e)}`;
      }
    },
    [listings, dialog, refresh],
  );

  return { url, listings, catalog: catalogOf(listings), error, refresh, publish, place, yank };
}
