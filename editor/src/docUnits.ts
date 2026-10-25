// The units a flow document carries, and the names of its composites. A unit keeps no name, so a
// composite inside another (expanded, ungrouped, or seen in a run) would show only its hash; the
// editor remembers what each composite is called on its canvases, a document carries the names
// of the composites it holds, and opening one teaches them back.

import { buildPreimage, type Evaluator, type ModuleEntry, type WUnit } from "./holon";
import { closureOf, fromHex, toHex } from "./flowDoc";

const names = new Map<string, string>();

/** What the composite `hash` is called, when known. */
export const nameOf = (hash: string): string | undefined => names.get(hash);

/** Every composite name known, by hash. */
export const knownNames = (): Record<string, string> => Object.fromEntries(names);

/** Remember that the composite `hash` is called `name` (a hash's own prefix is not a name). */
export function rememberName(hash: string, name: string) {
  if (name && name !== hash.slice(0, 8)) names.set(hash, name);
}

/** The units a document needs for composites `roots` (their whole closure): a module of a known
 *  engine (a js node's) as its entry, engine and params, everything else as its hex preimage; and
 *  the names known for any of them. `modules` is left out when there are none, so a flow without
 *  one saves as before. */
export function documentUnits(roots: string[], ev: Evaluator): { units: string[]; modules?: ModuleEntry[]; names: Record<string, string> } {
  const hashes = closureOf(roots, (h) => ev.get_unit(h) as WUnit);
  const known = hashes.flatMap((h) => (names.has(h) ? [[h, names.get(h)!] as const] : []));
  const units: string[] = [];
  const modules: ModuleEntry[] = [];
  for (const h of hashes) {
    const entry = ev.module_entry(h) as ModuleEntry | null | undefined;
    if (entry) modules.push(entry);
    else units.push(toHex(buildPreimage(ev.get_unit(h) as WUnit)));
  }
  return { units, ...(modules.length > 0 ? { modules } : {}), names: Object.fromEntries(known) };
}

/** A document's units put back into the Evaluator: each preimage inserted (re-hashed, so a
 *  tampered entry can only add a unit nobody points at), each module rebuilt from its engine and
 *  params and refused unless it comes out at the hash the document gave. Answers why some
 *  couldn't be restored, or null; the rest are restored regardless. */
export function restoreUnits(doc: { units?: string[]; modules?: ModuleEntry[] }, ev: Evaluator): string | null {
  let failed: string | null = null;
  for (const hex of doc.units ?? []) {
    try {
      ev.insert(fromHex(hex));
    } catch (e) {
      failed ??= e instanceof Error ? e.message : String(e);
    }
  }
  for (const m of doc.modules ?? []) {
    try {
      const hash = ev.insert_module(m.engine, m.params);
      if (hash !== m.hash) failed ??= `the module ${m.hash.slice(0, 12)}… rebuilds as ${hash.slice(0, 12)}…: another ${m.engine} engine`;
    } catch (e) {
      failed ??= e instanceof Error ? e.message : String(e);
    }
  }
  return failed;
}

/** A document's units put back into the Evaluator (restoreUnits), and its names remembered. */
export function restoreDocument(doc: { units?: string[]; modules?: ModuleEntry[]; names?: Record<string, string> }, ev: Evaluator): string | null {
  const failed = restoreUnits(doc, ev);
  for (const [hash, name] of Object.entries(doc.names ?? {})) if (!names.has(hash)) rememberName(hash, name);
  return failed;
}
