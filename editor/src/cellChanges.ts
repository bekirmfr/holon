// The rows of a cell's history, as CellHistory.tsx lists them: the runs that changed one cell, a
// collection or one item of it, newest first, each with its value before and after
// (`GET /deployments/<slug>/changes`, changes.rs). A restore or a deploy that moved it between two
// runs is a row of its own, so each value follows from the one below it.

import { formatValue } from "./diagnostics";
import type { WValue } from "./holon";
import type { CellChange, CellHistory, VersionEntry } from "./server";

export type CellRow = {
  key: string;
  /** The run that changed it, to open in view mode; none for a change between runs. */
  seq?: number;
  /** `run 41`, or `between runs 7 and 9`. */
  label: string;
  /** What the run was (`PUT /prices/tea · 200 · v2`), or what happened between (`restore: put back …`). */
  what: string;
  at_ms: number | null;
  /** `4 → 40`; for a whole collection, one per item it changed (`tea: 4 → 40`). */
  values: string[];
  failed: boolean;
};

/** A value as the history shows it; `(none)` for a cell not set, an item not there. */
export const shown = (v: WValue | null | undefined) => (v === null || v === undefined ? "(none)" : formatValue(v));

/** What the history is of, as the language writes it: `state.moves`, `balances["a"]`, `balances`. */
export function cellName(h: Pick<CellHistory, "name" | "item" | "collection">): string {
  if (h.item !== null && h.item !== undefined) return `${h.name}[${JSON.stringify(h.item)}]`;
  return h.collection ? h.name : `state.${h.name}`;
}

/** The version run `seq` ran on, its code `unit`: of the versions with that code, the last one
 *  deployed by then, not a later one that restored the same code (as history.rs names it). A
 *  version from before rc.30 doesn't say when it was deployed: then the newest of those. */
export function ranOn(versions: VersionEntry[], unit: string | undefined, seq: number): number | undefined {
  // Listed newest first.
  const same = versions.filter((v) => v.unit === unit);
  return (same.find((v) => v.first_seq != null && v.first_seq <= seq) ?? same.find((v) => v.first_seq == null) ?? same[0])?.number;
}

/** The rows, newest first, each run named with the version it ran on. */
export function cellRows(h: CellHistory, versions: VersionEntry[] = []): CellRow[] {
  return h.changes.map((c: CellChange, i): CellRow => {
    const values = c.items
      ? [...Object.entries(c.items).map(([k, v]) => `${k}: ${shown(v.before)} → ${shown(v.after)}`), ...(c.more_items ? ["and more"] : [])]
      : [`${shown(c.before)} → ${shown(c.after)}`];
    if (c.seq === undefined) {
      const [from, to] = c.between ?? [0, 0];
      const events = (c.events ?? []).map((e) => `${e.kind}: ${e.what}${e.by ? ` by ${e.by}` : ""}`);
      return { key: `b${from}-${to}-${i}`, label: `between runs ${from} and ${to}`, what: events.join("; ") || "outside a run", at_ms: c.at_ms ?? null, values, failed: false };
    }
    const n = ranOn(versions, c.unit, c.seq);
    const what = [c.method && c.url ? `${c.method} ${c.url}` : (c.kind ?? "run"), c.status ? String(c.status) : "", n ? `v${n}` : ""].filter(Boolean).join(" · ");
    return { key: `r${c.seq}`, seq: c.seq, label: `run ${c.seq}`, what: c.by ? `${what} · sent by ${c.by}` : what, at_ms: c.at_ms ?? null, values, failed: c.failed === true };
  });
}
