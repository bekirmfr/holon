// A flow's state, its page's State tab: the cells it keeps, each with its value now, and its
// collections, their items a page at a time. ⧗ beside any of them lists the runs that changed it
// (CellHistory.tsx). Read when shown, and again as it runs.

import { useEffect, useState } from "react";
import { CellHistoryButton } from "./CellHistory";
import { formatValue } from "./diagnostics";
import type { WValue } from "./holon";
import { isCollectionCell } from "./replay";
import { getCollectionItems, getDeploymentState, ServerError } from "./server";

const PAGE = 20;
const COLLECTION = "#collection:";

export function StateTab({ slug, version, onOpenRun }: { slug: string; version: number; onOpenRun: (seq: number) => void }) {
  const [state, setState] = useState<Record<string, WValue> | null>(null);
  const [error, setError] = useState("");
  const [stateless, setStateless] = useState(false);
  useEffect(() => {
    getDeploymentState(slug)
      .then((s) => {
        setState(s);
        setError("");
      })
      .catch((e: unknown) => {
        if (e instanceof ServerError && e.status === 400) setStateless(true);
        else setError(e instanceof Error ? e.message : String(e));
      });
  }, [slug, version]);
  if (stateless) return <p className="hl-flow__none">{slug} keeps no state: nothing it runs writes a cell or a collection.</p>;
  const keys = Object.keys(state ?? {}).sort();
  const cells = keys.filter((k) => !isCollectionCell(k));
  const collections = keys.filter(isCollectionCell).map((k) => k.slice(COLLECTION.length));
  return (
    <section className="hl-flow__state" aria-label="state">
      {error && <div className="hl-flow__bad">{error}</div>}
      {state && cells.length === 0 && collections.length === 0 && <p className="hl-flow__none">nothing kept yet</p>}
      {cells.length > 0 && (
        <table className="hl-table" aria-label="cells">
          <thead>
            <tr>
              <th>Cell</th>
              <th>Now</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {cells.map((k) => (
              <tr key={k}>
                <td className="hl-flow__what">{k}</td>
                <td className="hl-flow__value">{formatValue(state![k])}</td>
                <td className="hl-flow__cell-end">
                  <CellHistoryButton slug={slug} name={k} onOpenRun={onOpenRun} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {collections.map((name) => (
        <Collection key={name} slug={slug} name={name} version={version} onOpenRun={onOpenRun} />
      ))}
    </section>
  );
}

/** One collection: its items in key order, a page at a time, as it is now. */
function Collection({ slug, name, version, onOpenRun }: { slug: string; name: string; version: number; onOpenRun: (seq: number) => void }) {
  // The keys each page starts after (null: the first page), and the page shown.
  const [cursors, setCursors] = useState<(string | null)[]>([null]);
  const [page, setPage] = useState<{ items: [string, WValue][]; next: string | null } | null>(null);
  const [error, setError] = useState("");
  const at = cursors[cursors.length - 1];
  useEffect(() => {
    getCollectionItems(slug, name, { after: at, limit: PAGE })
      .then((r) => {
        setPage({ items: r.items, next: r.next ?? null });
        setError("");
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  }, [slug, name, at, version]);
  return (
    <div className="hl-flow__collection" role="group" aria-label={`collection ${name}`}>
      <div className="hl-flow__collection-head">
        <span className="hl-flow__what">{name}</span>
        <span className="hl-table__dim"> · collection{cursors.length > 1 ? ` · page ${cursors.length}` : ""}</span>
        <span className="hl-flow__spacer" />
        <CellHistoryButton slug={slug} name={name} collection onOpenRun={onOpenRun} />
      </div>
      {error && <div className="hl-flow__bad">{error}</div>}
      {page && page.items.length === 0 && <div className="hl-flow__none">empty</div>}
      {page && page.items.length > 0 && (
        <table className="hl-table">
          <tbody>
            {page.items.map(([k, v]) => (
              <tr key={k}>
                <td className="hl-flow__what">{k}</td>
                <td className="hl-flow__value">{formatValue(v)}</td>
                <td className="hl-flow__cell-end">
                  <CellHistoryButton slug={slug} name={name} item={k} onOpenRun={onOpenRun} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {(cursors.length > 1 || page?.next) && (
        <div className="hl-flow__bar">
          <button className="hl-flow__button" disabled={cursors.length === 1} onClick={() => setCursors((c) => c.slice(0, -1))}>
            ‹ earlier
          </button>
          <button className="hl-flow__button" disabled={!page?.next} onClick={() => page?.next && setCursors((c) => [...c, page.next])}>
            later ›
          </button>
        </div>
      )}
    </div>
  );
}
