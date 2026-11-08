// What the Library's chips say a sample does (gaps-web.md G25): each one a kind of thing a flow
// may do, read from the manifest items deploying it asks for (`HttpRequest("…")`, `DbRead("…")`).
// Answering requests isn't one: nearly every sample does.

export type Capability = { key: string; label: string; kinds: string[] };

/** The chips, in the order shown: "show flows that <label>". */
export const CAPABILITIES: Capability[] = [
  { key: "http", label: "call out", kinds: ["HttpRequest"] },
  { key: "db", label: "use a database", kinds: ["DbRead", "DbWrite"] },
  { key: "state", label: "keep state", kinds: ["StateRead", "StateWrite", "CollectionRead", "CollectionWrite"] },
  { key: "secret", label: "read secrets", kinds: ["SecretRead", "SecretAnswer"] },
  { key: "clock", label: "run on a clock", kinds: ["Clock"] },
  { key: "files", label: "use files", kinds: ["FileRead", "FileWrite", "FileArrival"] },
  { key: "wait", label: "wait for someone", kinds: ["Await"] },
  { key: "actions", label: "run another flow's actions", kinds: ["RunsAction"] },
  { key: "live", label: "push live updates", kinds: ["ChannelSend"] },
];

/** The kind of a manifest item: `DbRead` of `DbRead("shop/orders")`, `Clock` of `Clock`. */
export const kindOf = (item: string): string => /^[A-Za-z]+/.exec(item)?.[0] ?? item;

/** The chips' keys that a flow asking for `items` has, in the chips' order. */
export function capabilitiesOf(items: string[]): string[] {
  const kinds = new Set(items.map(kindOf));
  return CAPABILITIES.filter((c) => c.kinds.some((k) => kinds.has(k))).map((c) => c.key);
}

/** Whether a sample is shown: every chip picked is one it has, and the search is in its words. */
export function matches(sample: { title: string; description: string; shows: string[]; name: string }, has: string[], picked: string[], search: string): boolean {
  if (!picked.every((p) => has.includes(p))) return false;
  const q = search.trim().toLowerCase();
  if (q === "") return true;
  return [sample.title, sample.description, sample.name, ...sample.shows].some((t) => t.toLowerCase().includes(q));
}
