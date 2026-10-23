// The units a flow document carries, and the names of its composites. A unit keeps no name, so a
// composite inside another (expanded, ungrouped, or seen in a run) would show only its hash; the
// editor remembers what each composite is called on its canvases, a document carries the names
// of the composites it holds, and opening one teaches them back.

import { buildPreimage, type Evaluator, type WUnit } from "./holon";
import { closureOf, fromHex, toHex } from "./flowDoc";

const names = new Map<string, string>();

/** What the composite `hash` is called, when known. */
export const nameOf = (hash: string): string | undefined => names.get(hash);

/** Remember that the composite `hash` is called `name` (a hash's own prefix is not a name). */
export function rememberName(hash: string, name: string) {
  if (name && name !== hash.slice(0, 8)) names.set(hash, name);
}

/** The units a document needs for composites `roots` (their whole closure, as hex preimages),
 *  and the names known for any of them. */
export function documentUnits(roots: string[], ev: Evaluator): { units: string[]; names: Record<string, string> } {
  const hashes = closureOf(roots, (h) => ev.get_unit(h) as WUnit);
  const known = hashes.flatMap((h) => (names.has(h) ? [[h, names.get(h)!] as const] : []));
  return { units: hashes.map((h) => toHex(buildPreimage(ev.get_unit(h) as WUnit))), names: Object.fromEntries(known) };
}

/** A document's units put back into the Evaluator, and its names remembered. A preimage is
 *  re-hashed on insert, so a tampered entry can only add a unit nobody points at. Answers why
 *  some units couldn't be restored, or null; the rest are restored regardless. */
export function restoreDocument(doc: { units?: string[]; names?: Record<string, string> }, ev: Evaluator): string | null {
  let failed: string | null = null;
  for (const hex of doc.units ?? []) {
    try {
      ev.insert(fromHex(hex));
    } catch (e) {
      failed ??= e instanceof Error ? e.message : String(e);
    }
  }
  for (const [hash, name] of Object.entries(doc.names ?? {})) if (!names.has(hash)) rememberName(hash, name);
  return failed;
}
