// A flow's state, its page's State tab (web-console-L4.md §8.3): the cells it keeps, each with its
// value now, and its collections, their items a page at a time. ⧗ beside any of them opens its
// history inline, under it: the runs that changed it, and a line over time for a cell of numbers
// (CellHistory.tsx). Then the files its runs wrote, from what they did. Read when shown, and again
// as it runs.

import { Fragment, useEffect, useMemo, useState } from "react";
import { CellHistoryInline, useCellHistory } from "./CellHistory";
import { formatValue } from "./diagnostics";
import { filesWritten } from "./flowFacts";
import type { WValue } from "./holon";
import { isCollectionCell } from "./replay";
import { getCollectionItems, getDeploymentState, ServerError } from "./server";
import { useFlowLog } from "./useFlowLog";

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
  const keys = Object.keys(state ?? {}).sort();
  const cells = keys.filter((k) => !isCollectionCell(k));
  const collections = keys.filter(isCollectionCell).map((k) => k.slice(COLLECTION.length));
  return (
    <section className="hl-flow__state" aria-label="state">
      {stateless && <p className="hl-flow__none">{slug} keeps no state: nothing it runs writes a cell or a collection.</p>}
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
              <ValueRow key={k} slug={slug} name={k} value={state![k]} version={version} counted onOpenRun={onOpenRun} />
            ))}
          </tbody>
        </table>
      )}
      {collections.map((name) => (
        <Collection key={name} slug={slug} name={name} version={version} onOpenRun={onOpenRun} />
      ))}
      <Files slug={slug} onOpenRun={onOpenRun} />
    </section>
  );
}

/** A cell, or a collection's item, with its value now; ⧗ opens its history under it. `counted`:
 *  its changes are read at once, for ⧗ to say how many values it had. */
function ValueRow({ slug, name, item, value, version, counted = false, onOpenRun }: { slug: string; name: string; item?: string; value: WValue; version: number; counted?: boolean; onOpenRun: (seq: number) => void }) {
  const [open, setOpen] = useState(false);
  const { history, error } = useCellHistory(slug, name, item, open || counted, version);
  const n = history ? history.changes.length : null;
  const label = n === null ? "⧗" : `⧗ ${n}${history?.more ? "+" : ""} value${n === 1 ? "" : "s"}`;
  return (
    <Fragment>
      <tr>
        <td className="hl-flow__what">{item ?? name}</td>
        <td className="hl-flow__value">{formatValue(value)}</td>
        <td className="hl-flow__cell-end">
          <button className="hl-cellhistory__open" aria-expanded={open} aria-label={`history of ${item === undefined ? `state.${name}` : `${name}[${JSON.stringify(item)}]`}`} title="the runs that changed it" onClick={() => setOpen(!open)}>
            {label}
          </button>
        </td>
      </tr>
      {open && (
        <tr className="hl-flow__history-row">
          <td colSpan={3}>
            <CellHistoryInline history={history} error={error} onOpenRun={onOpenRun} />
          </td>
        </tr>
      )}
    </Fragment>
  );
}

/** One collection: its items in key order, a page at a time, as it is now. */
function Collection({ slug, name, version, onOpenRun }: { slug: string; name: string; version: number; onOpenRun: (seq: number) => void }) {
  // The keys each page starts after (null: the first page), and the page shown.
  const [cursors, setCursors] = useState<(string | null)[]>([null]);
  const [page, setPage] = useState<{ items: [string, WValue][]; next: string | null } | null>(null);
  const [error, setError] = useState("");
  const [open, setOpen] = useState(false);
  const whole = useCellHistory(slug, name, undefined, open, version);
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
        <button className="hl-cellhistory__open" aria-expanded={open} aria-label={`history of ${name}`} title="the runs that changed it" onClick={() => setOpen(!open)}>
          ⧗
        </button>
      </div>
      {open && <CellHistoryInline history={whole.history} error={whole.error} onOpenRun={onOpenRun} />}
      {error && <div className="hl-flow__bad">{error}</div>}
      {page && page.items.length === 0 && <div className="hl-flow__none">empty</div>}
      {page && page.items.length > 0 && (
        <table className="hl-table">
          <tbody>
            {page.items.map(([k, v]) => (
              <ValueRow key={k} slug={slug} name={name} item={k} value={v} version={version} onOpenRun={onOpenRun} />
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

/** The files its runs wrote (the runs its log holds here): each one's folder and name, how many
 *  runs wrote it, and the last, which opens. */
function Files({ slug, onOpenRun }: { slug: string; onOpenRun: (seq: number) => void }) {
  const { entries } = useFlowLog(slug);
  const files = useMemo(() => filesWritten(entries), [entries]);
  if (files.length === 0) return null;
  const did = { write: "wrote it", copy: "copied it over", delete: "deleted it" } as const;
  return (
    <section className="hl-flow__section" aria-label="files it wrote">
      <h2>Files it wrote</h2>
      <table className="hl-table">
        <thead>
          <tr>
            <th>File</th>
            <th>Runs</th>
            <th>The last</th>
          </tr>
        </thead>
        <tbody>
          {files.map((f) => (
            <tr key={`${f.folder}/${f.name}`}>
              <td className="hl-flow__what">
                {f.folder}/{f.name}
              </td>
              <td className="hl-table__num">{f.runs}</td>
              <td>
                <button className="hl-flow__link" onClick={() => onOpenRun(f.last)}>
                  run #{f.last}
                </button>{" "}
                <span className="hl-table__dim">{did[f.kind]}</span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {entries.length >= 500 && <p className="hl-flow__note">from the last 500 runs its log holds here</p>}
    </section>
  );
}
